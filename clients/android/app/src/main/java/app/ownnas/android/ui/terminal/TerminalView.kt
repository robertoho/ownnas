package app.ownnas.android.ui.terminal

import android.annotation.SuppressLint
import android.content.Context
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.Paint
import android.graphics.Typeface
import android.text.InputType
import android.util.TypedValue
import android.view.GestureDetector
import android.view.KeyEvent
import android.view.MotionEvent
import android.view.ScaleGestureDetector
import android.view.View
import android.view.inputmethod.BaseInputConnection
import android.view.inputmethod.EditorInfo
import android.view.inputmethod.InputConnection
import android.view.inputmethod.InputMethodManager
import app.ownnas.core.TerminalScreen
import app.ownnas.core.TerminalSession
import kotlin.math.abs
import kotlin.math.floor

/**
 * Draws a terminal session's screen and turns keys and gestures into terminal input. The
 * emulator itself runs in Rust; this view only paints its snapshots.
 */
@SuppressLint("ViewConstructor")
class TerminalView(context: Context, private val onSize: (rows: Int, cols: Int) -> Unit) : View(context) {
    var session: TerminalSession? = null
        set(value) {
            field = value
            if (rows > 0) value?.resize(rows.toUShort(), cols.toUShort())
            invalidate()
        }

    /** One-shot modifiers from the extra keys row, applied to the next key typed. */
    var ctrlArmed = false
    var altArmed = false
    var onModifiersUsed: () -> Unit = {}

    private val background = Color.parseColor("#151810")
    private val foreground = Color.parseColor("#F4F2E8")
    private val cursorColor = Color.parseColor("#DFF25A")

    private val paint = Paint(Paint.ANTI_ALIAS_FLAG).apply { typeface = Typeface.MONOSPACE }
    private val fill = Paint()
    private var textSizePx = TypedValue.applyDimension(TypedValue.COMPLEX_UNIT_SP, 13f, resources.displayMetrics)
    private var cellWidth = 0f
    private var cellHeight = 0f
    private var baseline = 0f
    private var rows = 0
    private var cols = 0
    private var scrollRemainder = 0f

    init {
        isFocusable = true
        isFocusableInTouchMode = true
        measureCells()
    }

    private fun measureCells() {
        paint.textSize = textSizePx
        cellWidth = paint.measureText("M")
        val metrics = paint.fontMetrics
        cellHeight = (metrics.descent - metrics.ascent + metrics.leading).let { if (it <= 0) textSizePx * 1.2f else it }
        baseline = -metrics.ascent
    }

    private fun fit() {
        if (width == 0 || height == 0) return
        val newCols = floor(width / cellWidth).toInt().coerceAtLeast(10)
        val newRows = floor(height / cellHeight).toInt().coerceAtLeast(4)
        if (newCols == cols && newRows == rows) return
        cols = newCols
        rows = newRows
        session?.resize(rows.toUShort(), cols.toUShort())
        onSize(rows, cols)
    }

    override fun onSizeChanged(w: Int, h: Int, oldw: Int, oldh: Int) {
        fit()
    }

    override fun onDraw(canvas: Canvas) {
        canvas.drawColor(background)
        val screen: TerminalScreen = session?.snapshot() ?: return
        screen.lines.forEachIndexed { row, runs ->
            val top = row * cellHeight
            for (run in runs) {
                val x = run.col.toInt() * cellWidth
                val runWidth = run.width.toInt() * cellWidth
                val bg = resolve(run.bg, background)
                if (bg != background) {
                    fill.color = bg
                    canvas.drawRect(x, top, x + runWidth, top + cellHeight, fill)
                }
                if (run.text.isBlank() && !run.underline) continue
                paint.color = resolve(run.fg, foreground)
                if (run.dim) paint.alpha = 0x99
                paint.isFakeBoldText = run.bold
                paint.textSkewX = if (run.italic) -0.2f else 0f
                paint.isUnderlineText = run.underline
                if (run.width.toInt() == run.text.codePointCount(0, run.text.length)) {
                    canvas.drawText(run.text, x, top + baseline, paint)
                } else {
                    // A wide character: center it in its two cells.
                    val drawn = paint.measureText(run.text)
                    canvas.drawText(run.text, x + (runWidth - drawn) / 2, top + baseline, paint)
                }
            }
        }
        paint.isFakeBoldText = false
        paint.textSkewX = 0f
        paint.isUnderlineText = false
        if (screen.cursorVisible && hasFocus()) {
            fill.color = cursorColor
            fill.alpha = 0xB0
            val x = screen.cursorCol.toInt() * cellWidth
            val y = screen.cursorRow.toInt() * cellHeight
            canvas.drawRect(x, y, x + cellWidth, y + cellHeight, fill)
            fill.alpha = 0xFF
        }
    }

    /** 0 is the default color, 1 and 2 the defaults swapped by inverse text. */
    private fun resolve(color: UInt, default: Int): Int = when (color) {
        0u -> default
        1u -> background
        2u -> foreground
        else -> color.toInt()
    }

    // Input

    fun showKeyboard() {
        requestFocus()
        context.getSystemService(InputMethodManager::class.java)?.showSoftInput(this, InputMethodManager.SHOW_IMPLICIT)
    }

    override fun onCheckIsTextEditor() = true

    override fun onCreateInputConnection(outAttrs: EditorInfo): InputConnection {
        // No suggestions or autocorrect: every key goes straight to the shell.
        outAttrs.inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_VISIBLE_PASSWORD or
            InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS
        outAttrs.imeOptions = EditorInfo.IME_FLAG_NO_FULLSCREEN or EditorInfo.IME_FLAG_NO_EXTRACT_UI or EditorInfo.IME_ACTION_NONE
        return object : BaseInputConnection(this, false) {
            override fun commitText(text: CharSequence, newCursorPosition: Int): Boolean {
                typed(text.toString().replace('\n', '\r'))
                return true
            }

            override fun deleteSurroundingText(beforeLength: Int, afterLength: Int): Boolean {
                repeat(beforeLength.coerceAtLeast(1)) { send(byteArrayOf(0x7f)) }
                return true
            }

            override fun sendKeyEvent(event: KeyEvent): Boolean {
                if (event.action == KeyEvent.ACTION_DOWN) onKeyDown(event.keyCode, event)
                return true
            }
        }
    }

    override fun onKeyDown(keyCode: Int, event: KeyEvent): Boolean {
        val application = session?.snapshot()?.applicationCursor == true
        fun arrow(letter: Char) = if (application) "\u001bO$letter" else "\u001b[$letter"
        val sequence = when (keyCode) {
            KeyEvent.KEYCODE_ENTER, KeyEvent.KEYCODE_NUMPAD_ENTER -> "\r"
            KeyEvent.KEYCODE_DEL -> "\u007f"
            KeyEvent.KEYCODE_FORWARD_DEL -> "\u001b[3~"
            KeyEvent.KEYCODE_TAB -> if (event.isShiftPressed) "\u001b[Z" else "\t"
            KeyEvent.KEYCODE_ESCAPE -> "\u001b"
            KeyEvent.KEYCODE_DPAD_UP -> arrow('A')
            KeyEvent.KEYCODE_DPAD_DOWN -> arrow('B')
            KeyEvent.KEYCODE_DPAD_RIGHT -> arrow('C')
            KeyEvent.KEYCODE_DPAD_LEFT -> arrow('D')
            KeyEvent.KEYCODE_MOVE_HOME -> "\u001b[H"
            KeyEvent.KEYCODE_MOVE_END -> "\u001b[F"
            KeyEvent.KEYCODE_PAGE_UP -> "\u001b[5~"
            KeyEvent.KEYCODE_PAGE_DOWN -> "\u001b[6~"
            else -> null
        }
        if (sequence != null) {
            send(sequence.toByteArray())
            return true
        }
        val unicode = event.getUnicodeChar(event.metaState and KeyEvent.META_CTRL_MASK.inv() and KeyEvent.META_ALT_MASK.inv())
        if (unicode == 0) return super.onKeyDown(keyCode, event)
        val text = String(Character.toChars(unicode))
        typed(text, ctrl = event.isCtrlPressed, alt = event.isAltPressed)
        return true
    }

    /** Sends typed text, applying Ctrl and Alt the way terminals do. */
    private fun typed(text: String, ctrl: Boolean = false, alt: Boolean = false) {
        val useCtrl = ctrl || ctrlArmed
        val useAlt = alt || altArmed
        if (ctrlArmed || altArmed) {
            ctrlArmed = false
            altArmed = false
            onModifiersUsed()
        }
        var out = text
        if (useCtrl && text.length == 1) {
            val c = text[0]
            val code = when (c) {
                in 'a'..'z' -> c - 'a' + 1
                in 'A'..'Z' -> c - 'A' + 1
                in '@'..'_' -> c - '@'
                ' ', '2' -> 0
                '/' -> 0x1f
                else -> -1
            }
            if (code >= 0) out = code.toChar().toString()
        }
        if (useAlt) out = "\u001b$out"
        session?.typeText(out)
    }

    fun send(bytes: ByteArray) {
        session?.sendBytes(bytes)
    }

    // Gestures: tap for the keyboard, drag to scroll back, pinch to resize the text.

    private val gestures = GestureDetector(
        context,
        object : GestureDetector.SimpleOnGestureListener() {
            override fun onSingleTapUp(e: MotionEvent): Boolean {
                showKeyboard()
                return true
            }

            override fun onScroll(e1: MotionEvent?, e2: MotionEvent, distanceX: Float, distanceY: Float): Boolean {
                if (abs(distanceY) < abs(distanceX)) return false
                scrollRemainder -= distanceY
                val lines = (scrollRemainder / cellHeight).toInt()
                if (lines != 0) {
                    scrollRemainder -= lines * cellHeight
                    session?.scrollBy(lines)
                    invalidate()
                }
                return true
            }
        },
    )

    private val scale = ScaleGestureDetector(
        context,
        object : ScaleGestureDetector.SimpleOnScaleGestureListener() {
            override fun onScale(detector: ScaleGestureDetector): Boolean {
                val min = TypedValue.applyDimension(TypedValue.COMPLEX_UNIT_SP, 8f, resources.displayMetrics)
                val max = TypedValue.applyDimension(TypedValue.COMPLEX_UNIT_SP, 28f, resources.displayMetrics)
                textSizePx = (textSizePx * detector.scaleFactor).coerceIn(min, max)
                measureCells()
                fit()
                invalidate()
                return true
            }
        },
    )

    @SuppressLint("ClickableViewAccessibility")
    override fun onTouchEvent(event: MotionEvent): Boolean {
        scale.onTouchEvent(event)
        if (!scale.isInProgress) gestures.onTouchEvent(event)
        return true
    }

    /** Text size in sp, to remember between terminals. */
    var textSizeSp: Float
        get() = textSizePx / resources.displayMetrics.scaledDensity
        set(value) {
            textSizePx = TypedValue.applyDimension(TypedValue.COMPLEX_UNIT_SP, value, resources.displayMetrics)
            measureCells()
            fit()
            invalidate()
        }
}
