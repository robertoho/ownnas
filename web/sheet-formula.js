/**
 * Spreadsheet formulas and number formats for the browser workbook editor: a parser and evaluator
 * for the common functions, Excel date serials, and Excel number format codes.
 * Formulas using anything unknown throw Unsupported, so the editor keeps the value the file saved.
 */

export class CellError {
  constructor(code) { this.code = code; }
  toString() { return this.code; }
}
export class Unsupported extends Error {}
class Fail extends Error {
  constructor(code) { super(code); this.code = code; }
}
const fail = code => { throw new Fail(code); };
const ERRORS = ['#DIV/0!', '#VALUE!', '#REF!', '#NAME?', '#NUM!', '#N/A', '#NULL!'];

// --- References ---------------------------------------------------------------------------------

export function columnNumber(letters) {
  let result = 0;
  for (const char of letters.toUpperCase()) result = result * 26 + char.charCodeAt(0) - 64;
  return result;
}

export function columnName(column) {
  let value = column, result = '';
  while (value > 0) {
    const remainder = (value - 1) % 26;
    result = String.fromCharCode(65 + remainder) + result;
    value = Math.floor((value - 1) / 26);
  }
  return result || 'A';
}

export function parseRef(ref) {
  const match = /^\$?([A-Z]{1,3})\$?([1-9]\d*)$/i.exec(ref || '');
  if (!match) return null;
  const row = Number(match[2]), col = columnNumber(match[1]);
  if (row > 1048576 || col > 16384) return null;
  return { row, col };
}

export const refName = (row, col) => `${columnName(col)}${row}`;

function refPart(text) {
  const match = /^(\$?)([A-Z]{1,3})?(\$?)(\d+)?$/i.exec(text);
  if (!match) return null;
  return {
    col: match[2] ? columnNumber(match[2]) : null, absCol: !!match[1],
    row: match[4] ? Number(match[4]) : null, absRow: !!match[3],
  };
}

// --- Tokens -------------------------------------------------------------------------------------

const SHEET = String.raw`(?:'(?:[^']|'')+'|[A-Za-z_À-￿][\w.À-￿]*)!`;
const CELL = String.raw`\$?[A-Za-z]{1,3}\$?\d+`;
const TOKENS = new RegExp([
  String.raw`(\s+)`,
  String.raw`("(?:[^"]|"")*")`,
  String.raw`((?:${SHEET})?(?:${CELL}(?::${CELL})?|\$?[A-Za-z]{1,3}:\$?[A-Za-z]{1,3}|\$?\d+:\$?\d+))(?![\w(])`,
  String.raw`(\d+(?:\.\d*)?(?:[eE][-+]?\d+)?|\.\d+(?:[eE][-+]?\d+)?)`,
  String.raw`(#(?:DIV/0!|VALUE!|REF!|NAME\?|NUM!|N/A|NULL!))`,
  String.raw`([A-Za-z_][\w.]*)(?=\s*\()`,
  String.raw`(<=|>=|<>|[-+*/^&=<>%(),;{}])`,
  String.raw`(${SHEET}#REF!|#REF!|[A-Za-z_\\][\w.]*)`,
].join('|'), 'y');

export function tokenize(formula) {
  const tokens = [];
  TOKENS.lastIndex = 0;
  while (TOKENS.lastIndex < formula.length) {
    const start = TOKENS.lastIndex;
    const match = TOKENS.exec(formula);
    if (!match) throw new Unsupported(`Unexpected "${formula.slice(start, start + 10)}"`);
    const [text, space, str, ref, num, err, fn, op, name] = match;
    if (space) continue;
    if (str) tokens.push({ t: 'str', v: str.slice(1, -1).replace(/""/g, '"'), start, text });
    else if (ref) tokens.push({ t: 'ref', ...splitRef(ref), start, text });
    else if (num) tokens.push({ t: 'num', v: Number(num), start, text });
    else if (err) tokens.push({ t: 'err', v: err.toUpperCase(), start, text });
    else if (fn) tokens.push({ t: 'fn', v: fn.toUpperCase().replace(/^_XLFN\.|^_XLWS\./, ''), start, text });
    else if (op) tokens.push({ t: 'op', v: op, start, text });
    else if (/^(TRUE|FALSE)$/i.test(name)) tokens.push({ t: 'bool', v: name.toUpperCase() === 'TRUE', start, text });
    else tokens.push({ t: 'name', v: name, start, text });
  }
  return tokens;
}

function splitRef(text) {
  const bang = text.lastIndexOf('!');
  let sheet = null, body = text;
  if (bang > 0) {
    sheet = text.slice(0, bang);
    if (sheet.startsWith("'")) sheet = sheet.slice(1, -1).replace(/''/g, "'");
    body = text.slice(bang + 1);
  }
  const [a, b] = body.split(':');
  return { sheet, a: refPart(a), b: b ? refPart(b) : null };
}

function partText(part) {
  return `${part.col != null ? (part.absCol ? '$' : '') + columnName(part.col) : ''}${part.row != null ? (part.absRow ? '$' : '') + part.row : ''}`;
}

/** Moves a formula's relative references, as copying it or filling a shared formula does. */
export function shiftFormula(formula, dRow, dCol) {
  if (!dRow && !dCol) return formula;
  let tokens;
  try { tokens = tokenize(formula); } catch { return formula; }
  let out = '', at = 0;
  for (const token of tokens) {
    if (token.t !== 'ref') continue;
    const move = part => {
      const next = { ...part };
      if (next.col != null && !next.absCol) next.col += dCol;
      if (next.row != null && !next.absRow) next.row += dRow;
      return next;
    };
    const a = move(token.a), b = token.b && move(token.b);
    const broken = [a, b].some(part => part && ((part.col != null && (part.col < 1 || part.col > 16384)) || (part.row != null && (part.row < 1 || part.row > 1048576))));
    const bang = token.text.lastIndexOf('!');
    const prefix = bang > 0 ? token.text.slice(0, bang + 1) : '';
    out += formula.slice(at, token.start) + (broken ? `${prefix}#REF!` : prefix + partText(a) + (b ? `:${partText(b)}` : ''));
    at = token.start + token.text.length;
  }
  return out + formula.slice(at);
}

/** Where the span lo…hi of rows or columns goes when `count` are inserted before `at` (count > 0)
 * or deleted from `at` on (count < 0): [lo, hi], or null when all of it is deleted. */
export function moveSpan(lo, hi, at, count) {
  if (count > 0) return [lo >= at ? lo + count : lo, hi >= at ? hi + count : hi];
  const end = at - count - 1;
  if (lo >= at && hi <= end) return null;
  const left = lo < at ? lo : lo > end ? lo + count : at;
  const right = hi < at ? hi : hi > end ? hi + count : at - 1;
  return right < left ? null : [left, right];
}

/** A reference's two ends moved for rows (`axis` "row") or columns inserted or deleted, or null. */
function moveParts(a, b, axis, at, count) {
  const end = b || a;
  if (a[axis] == null || end[axis] == null) return { a, b };
  const lo = Math.min(a[axis], end[axis]), hi = Math.max(a[axis], end[axis]);
  const span = moveSpan(lo, hi, at, count);
  if (!span) return null;
  const flip = a[axis] > end[axis];
  const first = { ...a, [axis]: flip ? span[1] : span[0] };
  if (!b) return { a: first, b: null };
  return { a: first, b: { ...b, [axis]: flip ? span[0] : span[1] } };
}

/** A formula's references to `sheet` (or, with `here`, to its own sheet) moved for rows or
 * columns inserted or deleted there; references to deleted cells become #REF!. */
export function moveFormula(formula, { sheet, here, axis, at, count }) {
  let tokens;
  try { tokens = tokenize(formula); } catch { return formula; }
  let out = '', from = 0;
  for (const token of tokens) {
    if (token.t !== 'ref') continue;
    if (token.sheet == null ? !here : token.sheet.toLowerCase() !== sheet.toLowerCase()) continue;
    const moved = moveParts(token.a, token.b, axis, at, count);
    const bang = token.text.lastIndexOf('!');
    const prefix = bang > 0 ? token.text.slice(0, bang + 1) : '';
    out += formula.slice(from, token.start) + (moved ? prefix + partText(moved.a) + (moved.b ? `:${partText(moved.b)}` : '') : `${prefix}#REF!`);
    from = token.start + token.text.length;
  }
  return out + formula.slice(from);
}

/** A space-separated list of cells and ranges ("A1 B2:C4"), moved the same way; deleted ones are
 * left out, so the result can be empty. */
export function moveSqref(sqref, axis, at, count) {
  return String(sqref || '').split(/\s+/).filter(Boolean).map(item => {
    const [a, b] = item.split(':').map(refPart);
    if (!a) return item;
    const moved = moveParts(a, b, axis, at, count);
    return moved ? partText(moved.a) + (moved.b ? `:${partText(moved.b)}` : '') : '';
  }).filter(Boolean).join(' ');
}

// --- Parser -------------------------------------------------------------------------------------

const BINARY = { '=': 1, '<>': 1, '<': 1, '>': 1, '<=': 1, '>=': 1, '&': 2, '+': 3, '-': 3, '*': 4, '/': 4, '^': 5 };

export function parseFormula(formula) {
  const tokens = tokenize(formula);
  let at = 0;
  const peek = () => tokens[at];
  const isOp = (value) => peek()?.t === 'op' && peek().v === value;
  const expect = (value) => {
    if (!isOp(value)) throw new Unsupported(`Expected ${value}`);
    at++;
  };
  function primary() {
    const token = tokens[at++];
    if (!token) throw new Unsupported('Formula ends early');
    if (token.t === 'num' || token.t === 'str' || token.t === 'bool') return { k: 'value', v: token.v };
    if (token.t === 'err') return { k: 'value', v: new CellError(token.v) };
    if (token.t === 'ref') return { k: 'ref', sheet: token.sheet, a: token.a, b: token.b };
    if (token.t === 'name') {
      if (/#REF!$/i.test(token.v)) return { k: 'value', v: new CellError('#REF!') };
      throw new Unsupported(`Names like ${token.v} are not supported`);
    }
    if (token.t === 'fn') {
      expect('(');
      const args = [];
      if (!isOp(')')) {
        for (;;) {
          if (isOp(',') || isOp(';') || isOp(')')) args.push({ k: 'missing' });
          else args.push(expression(0));
          if (isOp(',') || isOp(';')) { at++; continue; }
          break;
        }
      }
      expect(')');
      return { k: 'call', name: token.v, args };
    }
    if (token.t === 'op' && token.v === '(') {
      const inner = expression(0);
      expect(')');
      return inner;
    }
    if (token.t === 'op' && (token.v === '-' || token.v === '+')) {
      return { k: 'unary', op: token.v, x: postfix(primary()) };
    }
    throw new Unsupported(`Unexpected ${token.text}`);
  }
  function postfix(node) {
    while (isOp('%')) { at++; node = { k: 'percent', x: node }; }
    return node;
  }
  function expression(min) {
    let left = postfix(primary());
    for (;;) {
      const token = peek();
      const prec = token?.t === 'op' ? BINARY[token.v] : undefined;
      if (prec === undefined || prec <= min) break;
      at++;
      left = { k: 'binary', op: token.v, a: left, b: expression(prec) };
    }
    return left;
  }
  const tree = expression(0);
  if (at !== tokens.length) throw new Unsupported(`Unexpected ${tokens[at].text}`);
  return tree;
}

// --- Values -------------------------------------------------------------------------------------

class Range {
  constructor(rows) { this.rows = rows; }
  *values() { for (const row of this.rows) yield* row; }
  get height() { return this.rows.length; }
  get width() { return this.rows[0]?.length || 0; }
}

function scalar(value) {
  if (value instanceof Range) {
    if (value.height === 1 && value.width === 1) return value.rows[0][0];
    fail('#VALUE!');
  }
  return value;
}

function num(value) {
  value = scalar(value);
  if (value instanceof CellError) throw new Fail(value.code);
  if (value == null || value === '') return 0;
  if (typeof value === 'number') return value;
  if (typeof value === 'boolean') return value ? 1 : 0;
  const parsed = parseNumber(String(value));
  if (parsed == null) fail('#VALUE!');
  return parsed;
}

function text(value) {
  value = scalar(value);
  if (value instanceof CellError) throw new Fail(value.code);
  if (value == null) return '';
  if (typeof value === 'boolean') return value ? 'TRUE' : 'FALSE';
  if (typeof value === 'number') return formatGeneral(value, { decimal: '.' });
  return String(value);
}

function bool(value) {
  value = scalar(value);
  if (value instanceof CellError) throw new Fail(value.code);
  if (typeof value === 'boolean') return value;
  if (value == null || value === '') return false;
  if (typeof value === 'number') return value !== 0;
  if (/^true$/i.test(value)) return true;
  if (/^false$/i.test(value)) return false;
  fail('#VALUE!');
}

function parseNumber(value) {
  const trimmed = value.trim();
  if (!/^[-+]?(?:\d+(?:\.\d*)?|\.\d+)(?:e[-+]?\d+)?%?$/i.test(trimmed)) return null;
  return trimmed.endsWith('%') ? Number(trimmed.slice(0, -1)) / 100 : Number(trimmed);
}

function rank(value) {
  return typeof value === 'number' ? 0 : typeof value === 'string' ? 1 : typeof value === 'boolean' ? 2 : 0;
}

function compare(a, b) {
  if (a == null) a = typeof b === 'string' ? '' : typeof b === 'boolean' ? false : 0;
  if (b == null) b = typeof a === 'string' ? '' : typeof a === 'boolean' ? false : 0;
  if (rank(a) !== rank(b)) return rank(a) - rank(b);
  if (typeof a === 'string') {
    const x = a.toLowerCase(), y = b.toLowerCase();
    return x < y ? -1 : x > y ? 1 : 0;
  }
  return a < b ? -1 : a > b ? 1 : 0;
}

function finite(value) {
  if (!Number.isFinite(value)) fail('#NUM!');
  return value;
}

// --- Evaluation ---------------------------------------------------------------------------------

/**
 * Evaluates a parsed formula. `context` gives `cell(sheet, row, col)` for a cell's value (sheet null
 * for the formula's own sheet), `size(sheet)` for a sheet's used rows and columns, `row` and `col`
 * for the formula's cell, and `date1904`.
 */
export function evaluate(tree, context) {
  try {
    const value = scalar(run(tree, context));
    if (typeof value === 'number' && !Number.isFinite(value)) return new CellError('#NUM!');
    return value == null ? 0 : value;
  } catch (error) {
    if (error instanceof Fail) return new CellError(error.code);
    throw error;
  }
}

function run(node, context) {
  switch (node.k) {
    case 'value': return node.v;
    case 'missing': return null;
    case 'ref': return reference(node, context);
    case 'unary': {
      const value = num(run(node.x, context));
      return node.op === '-' ? -value : value;
    }
    case 'percent': return num(run(node.x, context)) / 100;
    case 'binary': return binary(node, context);
    case 'call': {
      const fn = FUNCTIONS[node.name];
      if (!fn) throw new Unsupported(`${node.name} is not supported`);
      return fn(node.args, context);
    }
    default: throw new Unsupported('Unknown formula part');
  }
}

function reference(node, context) {
  const size = context.size(node.sheet);
  if (!size) fail('#REF!');
  const a = node.a, b = node.b || node.a;
  const r1 = a.row ?? 1, r2 = b.row ?? Math.max(1, size.rows);
  const c1 = a.col ?? 1, c2 = b.col ?? Math.max(1, size.cols);
  if (!node.b && a.row != null && a.col != null) return context.cell(node.sheet, a.row, a.col);
  const top = Math.min(r1, r2), bottom = Math.min(Math.max(r1, r2), Math.max(top, size.rows));
  const left = Math.min(c1, c2), right = Math.min(Math.max(c1, c2), Math.max(left, size.cols));
  if ((bottom - top + 1) * (right - left + 1) > 2_000_000) throw new Unsupported('Range is too large');
  const rows = [];
  for (let row = top; row <= bottom; row++) {
    const line = [];
    for (let col = left; col <= right; col++) line.push(context.cell(node.sheet, row, col));
    rows.push(line);
  }
  return new Range(rows);
}

function binary(node, context) {
  const a = scalar(run(node.a, context)), b = scalar(run(node.b, context));
  if (a instanceof CellError) throw new Fail(a.code);
  if (b instanceof CellError) throw new Fail(b.code);
  switch (node.op) {
    case '+': return num(a) + num(b);
    case '-': return num(a) - num(b);
    case '*': return num(a) * num(b);
    case '/': {
      const divisor = num(b);
      if (divisor === 0) fail('#DIV/0!');
      return num(a) / divisor;
    }
    case '^': return finite(Math.pow(num(a), num(b)));
    case '&': return text(a) + text(b);
    case '=': return compare(a, b) === 0;
    case '<>': return compare(a, b) !== 0;
    case '<': return compare(a, b) < 0;
    case '>': return compare(a, b) > 0;
    case '<=': return compare(a, b) <= 0;
    case '>=': return compare(a, b) >= 0;
    default: throw new Unsupported(node.op);
  }
}

/** The numbers among the arguments: from ranges only real numbers, given directly anything numeric. */
function numbers(args, context, { all = false } = {}) {
  const out = [];
  for (const arg of args) {
    if (arg.k === 'missing') continue;
    const value = run(arg, context);
    if (value instanceof Range || arg.k === 'ref') {
      const items = value instanceof Range ? value.values() : [value];
      for (const item of items) {
        if (item instanceof CellError) throw new Fail(item.code);
        if (typeof item === 'number') out.push(item);
        else if (all && typeof item === 'boolean') out.push(item ? 1 : 0);
        else if (all && typeof item === 'string') out.push(0);
      }
    } else out.push(num(value));
  }
  return out;
}

function flat(args, context) {
  const out = [];
  for (const arg of args) {
    const value = run(arg, context);
    if (value instanceof Range) out.push(...value.values());
    else out.push(value);
  }
  return out;
}

function range(node, context) {
  const value = run(node, context);
  return value instanceof Range ? value : new Range([[value]]);
}

const argNum = (args, i, context, fallback) => (args[i] && args[i].k !== 'missing' ? num(run(args[i], context)) : fallback);
const argText = (args, i, context, fallback = '') => (args[i] && args[i].k !== 'missing' ? text(run(args[i], context)) : fallback);
const need = (args, min, max = min) => { if (args.length < min || args.length > max) fail('#VALUE!'); };

function roundTo(value, digits, mode) {
  const factor = Math.pow(10, digits);
  const scaled = Math.abs(value) * factor;
  const corrected = Number(scaled.toPrecision(15));
  const rounded = mode === 'up' ? Math.ceil(corrected) : mode === 'down' ? Math.floor(corrected) : Math.round(corrected);
  return Math.sign(value) * rounded / factor;
}

function criteria(raw) {
  if (typeof raw === 'number' || typeof raw === 'boolean') return value => compare(value, raw) === 0 && rank(value) === rank(raw);
  const match = /^(<=|>=|<>|<|>|=)?(.*)$/s.exec(String(raw ?? ''));
  const op = match[1] || '=', operand = match[2];
  const number = parseNumber(operand);
  const target = number != null ? number : /^(true|false)$/i.test(operand) ? /^true$/i.test(operand) : operand;
  let pattern = null;
  if (typeof target === 'string' && /[*?]/.test(target) && (op === '=' || op === '<>')) {
    pattern = new RegExp(`^${target.replace(/~([*?~])|([*?])|[.+^${}()|[\]\\]/g, (all, escaped, wild) => escaped ? `\\${escaped}` : wild === '*' ? '.*' : wild === '?' ? '.' : `\\${all}`)}$`, 'is');
  }
  return value => {
    if (value instanceof CellError) return false;
    if (pattern) return (typeof value === 'string' && pattern.test(value)) === (op === '=');
    if (target === '' && op === '=') return value == null || value === '';
    if (target === '' && op === '<>') return !(value == null || value === '');
    if (op !== '=' && op !== '<>' && rank(value ?? target) !== rank(target)) return false;
    if (value == null) value = typeof target === 'string' ? '' : 0;
    if ((op === '=' || op === '<>') && rank(value) !== rank(target)) return op === '<>';
    const order = compare(value, target);
    return { '=': order === 0, '<>': order !== 0, '<': order < 0, '>': order > 0, '<=': order <= 0, '>=': order >= 0 }[op];
  };
}

function ifs(args, context, start) {
  const pairs = [];
  for (let i = start; i < args.length; i += 2) {
    const where = range(args[i], context);
    pairs.push({ where, test: criteria(scalarOf(run(args[i + 1], context))) });
  }
  return pairs;
}

const scalarOf = value => (value instanceof Range ? value.rows[0]?.[0] : value);

function matching(pairs, index) {
  return pairs.every(({ where, test }) => {
    const width = where.width, row = Math.floor(index / width), col = index % width;
    return test(where.rows[row]?.[col]);
  });
}

function lookupIndex(list, target, mode) {
  if (mode === 0) {
    const test = criteria(typeof target === 'string' ? target : target);
    return list.findIndex(value => (typeof target === 'string' ? test(value) : compare(value, target) === 0 && rank(value) === rank(target)));
  }
  let found = -1;
  for (let i = 0; i < list.length; i++) {
    const value = list[i];
    if (value == null || rank(value) !== rank(target)) continue;
    const order = compare(value, target);
    if (mode > 0 ? order <= 0 : order >= 0) found = i;
    else break;
  }
  return found;
}

const FUNCTIONS = {
  SUM: (args, c) => numbers(args, c).reduce((a, b) => a + b, 0),
  PRODUCT: (args, c) => numbers(args, c).reduce((a, b) => a * b, 1),
  AVERAGE: (args, c) => { const list = numbers(args, c); if (!list.length) fail('#DIV/0!'); return list.reduce((a, b) => a + b, 0) / list.length; },
  AVERAGEA: (args, c) => { const list = numbers(args, c, { all: true }); if (!list.length) fail('#DIV/0!'); return list.reduce((a, b) => a + b, 0) / list.length; },
  MIN: (args, c) => { const list = numbers(args, c); return list.length ? Math.min(...list) : 0; },
  MAX: (args, c) => { const list = numbers(args, c); return list.length ? Math.max(...list) : 0; },
  MEDIAN: (args, c) => {
    const list = numbers(args, c).sort((a, b) => a - b);
    if (!list.length) fail('#NUM!');
    const mid = Math.floor(list.length / 2);
    return list.length % 2 ? list[mid] : (list[mid - 1] + list[mid]) / 2;
  },
  LARGE: (args, c) => { need(args, 2); const list = numbers([args[0]], c).sort((a, b) => b - a); const k = argNum(args, 1, c); if (k < 1 || k > list.length) fail('#NUM!'); return list[Math.ceil(k) - 1]; },
  SMALL: (args, c) => { need(args, 2); const list = numbers([args[0]], c).sort((a, b) => a - b); const k = argNum(args, 1, c); if (k < 1 || k > list.length) fail('#NUM!'); return list[Math.ceil(k) - 1]; },
  COUNT: (args, c) => {
    let count = 0;
    for (const arg of args) {
      const value = run(arg, c);
      for (const item of value instanceof Range ? value.values() : [value]) {
        if (typeof item === 'number' || (!(value instanceof Range) && arg.k !== 'ref' && parseNumber(String(item ?? 'x')) != null)) count++;
      }
    }
    return count;
  },
  COUNTA: (args, c) => flat(args, c).filter(value => value != null && value !== '').length,
  COUNTBLANK: (args, c) => flat(args, c).filter(value => value == null || value === '').length,
  ABS: (args, c) => { need(args, 1); return Math.abs(argNum(args, 0, c)); },
  INT: (args, c) => { need(args, 1); return Math.floor(argNum(args, 0, c)); },
  TRUNC: (args, c) => { need(args, 1, 2); return roundTo(argNum(args, 0, c), argNum(args, 1, c, 0), 'down'); },
  ROUND: (args, c) => { need(args, 2); return roundTo(argNum(args, 0, c), Math.trunc(argNum(args, 1, c)), 'half'); },
  ROUNDUP: (args, c) => { need(args, 2); return roundTo(argNum(args, 0, c), Math.trunc(argNum(args, 1, c)), 'up'); },
  ROUNDDOWN: (args, c) => { need(args, 2); return roundTo(argNum(args, 0, c), Math.trunc(argNum(args, 1, c)), 'down'); },
  MOD: (args, c) => { need(args, 2); const n = argNum(args, 0, c), d = argNum(args, 1, c); if (d === 0) fail('#DIV/0!'); return n - d * Math.floor(n / d); },
  POWER: (args, c) => { need(args, 2); return finite(Math.pow(argNum(args, 0, c), argNum(args, 1, c))); },
  SQRT: (args, c) => { need(args, 1); const n = argNum(args, 0, c); if (n < 0) fail('#NUM!'); return Math.sqrt(n); },
  SIGN: (args, c) => { need(args, 1); return Math.sign(argNum(args, 0, c)); },
  PI: () => Math.PI,
  CEILING: (args, c) => { const n = argNum(args, 0, c), s = argNum(args, 1, c, 1); return s === 0 ? 0 : Math.ceil(n / s) * s; },
  FLOOR: (args, c) => { const n = argNum(args, 0, c), s = argNum(args, 1, c, 1); if (s === 0) fail('#DIV/0!'); return Math.floor(n / s) * s; },
  IF: (args, c) => {
    need(args, 1, 3);
    if (bool(run(args[0], c))) return args.length > 1 ? run(args[1], c) ?? 0 : true;
    return args.length > 2 ? run(args[2], c) ?? 0 : false;
  },
  IFERROR: (args, c) => {
    need(args, 2);
    try {
      const value = scalar(run(args[0], c));
      if (value instanceof CellError) return run(args[1], c);
      return value;
    } catch (error) {
      if (error instanceof Fail) return run(args[1], c);
      throw error;
    }
  },
  IFNA: (args, c) => {
    need(args, 2);
    try {
      const value = scalar(run(args[0], c));
      return value instanceof CellError && value.code === '#N/A' ? run(args[1], c) : value;
    } catch (error) {
      if (error instanceof Fail && error.code === '#N/A') return run(args[1], c);
      throw error;
    }
  },
  AND: (args, c) => flat(args, c).filter(v => v != null && typeof v !== 'string').every(bool),
  OR: (args, c) => flat(args, c).filter(v => v != null && typeof v !== 'string').some(bool),
  XOR: (args, c) => flat(args, c).filter(v => v != null && typeof v !== 'string').filter(bool).length % 2 === 1,
  NOT: (args, c) => { need(args, 1); return !bool(run(args[0], c)); },
  TRUE: () => true,
  FALSE: () => false,
  NA: () => fail('#N/A'),
  CHOOSE: (args, c) => { const i = Math.trunc(argNum(args, 0, c)); if (i < 1 || i >= args.length) fail('#VALUE!'); return run(args[i], c); },
  CONCATENATE: (args, c) => args.map((arg, i) => argText(args, i, c)).join(''),
  CONCAT: (args, c) => flat(args, c).map(text).join(''),
  TEXTJOIN: (args, c) => {
    const separator = argText(args, 0, c), skip = bool(run(args[1], c));
    return flat(args.slice(2), c).map(text).filter(value => !skip || value !== '').join(separator);
  },
  LEN: (args, c) => { need(args, 1); return argText(args, 0, c).length; },
  LEFT: (args, c) => argText(args, 0, c).slice(0, Math.max(0, argNum(args, 1, c, 1))),
  RIGHT: (args, c) => { const s = argText(args, 0, c), n = Math.max(0, argNum(args, 1, c, 1)); return n ? s.slice(-n) : ''; },
  MID: (args, c) => { need(args, 3); const start = argNum(args, 1, c); if (start < 1) fail('#VALUE!'); return argText(args, 0, c).substr(start - 1, Math.max(0, argNum(args, 2, c))); },
  UPPER: (args, c) => argText(args, 0, c).toUpperCase(),
  LOWER: (args, c) => argText(args, 0, c).toLowerCase(),
  PROPER: (args, c) => argText(args, 0, c).toLowerCase().replace(/(^|[^\p{L}])(\p{L})/gu, (all, before, letter) => before + letter.toUpperCase()),
  TRIM: (args, c) => argText(args, 0, c).replace(/ +/g, ' ').trim(),
  REPT: (args, c) => argText(args, 0, c).repeat(Math.max(0, Math.min(32767, argNum(args, 1, c)))),
  EXACT: (args, c) => argText(args, 0, c) === argText(args, 1, c),
  SUBSTITUTE: (args, c) => {
    const s = argText(args, 0, c), from = argText(args, 1, c), to = argText(args, 2, c);
    if (!from) return s;
    if (args.length < 4) return s.split(from).join(to);
    const nth = argNum(args, 3, c);
    let index = -1;
    for (let i = 0; i < nth; i++) { index = s.indexOf(from, index + 1); if (index < 0) return s; }
    return s.slice(0, index) + to + s.slice(index + from.length);
  },
  FIND: (args, c) => { const at = argText(args, 1, c).indexOf(argText(args, 0, c), argNum(args, 2, c, 1) - 1); if (at < 0) fail('#VALUE!'); return at + 1; },
  SEARCH: (args, c) => { const at = argText(args, 1, c).toLowerCase().indexOf(argText(args, 0, c).toLowerCase(), argNum(args, 2, c, 1) - 1); if (at < 0) fail('#VALUE!'); return at + 1; },
  VALUE: (args, c) => { const value = scalar(run(args[0], c)); if (typeof value === 'number') return value; const parsed = parseNumber(text(value)); if (parsed == null) fail('#VALUE!'); return parsed; },
  TEXT: (args, c) => {
    const value = scalar(run(args[0], c));
    if (value instanceof CellError) throw new Fail(value.code);
    return formatValue(typeof value === 'string' && parseNumber(value) != null ? parseNumber(value) : value, argText(args, 1, c), { date1904: c.date1904, decimal: '.', group: ',' }).text;
  },
  ISBLANK: (args, c) => { const v = scalar(run(args[0], c)); return v == null; },
  ISNUMBER: (args, c) => typeof scalar(run(args[0], c)) === 'number',
  ISTEXT: (args, c) => typeof scalar(run(args[0], c)) === 'string',
  ISLOGICAL: (args, c) => typeof scalar(run(args[0], c)) === 'boolean',
  ISERROR: (args, c) => { try { return scalar(run(args[0], c)) instanceof CellError; } catch (e) { if (e instanceof Fail) return true; throw e; } },
  ISNA: (args, c) => { try { const v = scalar(run(args[0], c)); return v instanceof CellError && v.code === '#N/A'; } catch (e) { if (e instanceof Fail) return e.code === '#N/A'; throw e; } },
  DATE: (args, c) => { need(args, 3); return dateSerial(argNum(args, 0, c), argNum(args, 1, c), argNum(args, 2, c), c.date1904); },
  YEAR: (args, c) => serialParts(argNum(args, 0, c), c.date1904).year,
  MONTH: (args, c) => serialParts(argNum(args, 0, c), c.date1904).month,
  DAY: (args, c) => serialParts(argNum(args, 0, c), c.date1904).day,
  HOUR: (args, c) => serialParts(argNum(args, 0, c), c.date1904).hours,
  MINUTE: (args, c) => serialParts(argNum(args, 0, c), c.date1904).minutes,
  SECOND: (args, c) => serialParts(argNum(args, 0, c), c.date1904).seconds,
  WEEKDAY: (args, c) => {
    const day = serialParts(argNum(args, 0, c), c.date1904).weekday, type = argNum(args, 1, c, 1);
    if (type === 2) return ((day + 6) % 7) + 1;
    if (type === 3) return (day + 6) % 7;
    return day + 1;
  },
  ROW: (args, c) => (args.length && args[0].k === 'ref' ? args[0].a.row ?? c.row : c.row),
  COLUMN: (args, c) => (args.length && args[0].k === 'ref' ? args[0].a.col ?? c.col : c.col),
  SUMIF: (args, c) => {
    need(args, 2, 3);
    const where = range(args[0], c), test = criteria(scalarOf(run(args[1], c))), sum = args[2] ? range(args[2], c) : where;
    let total = 0;
    where.rows.forEach((row, r) => row.forEach((value, col) => { const add = sum.rows[r]?.[col]; if (test(value) && typeof add === 'number') total += add; }));
    return total;
  },
  SUMIFS: (args, c) => {
    const sum = range(args[0], c), pairs = ifs(args, c, 1);
    let total = 0;
    [...sum.values()].forEach((value, i) => { if (typeof value === 'number' && matching(pairs, i)) total += value; });
    return total;
  },
  COUNTIF: (args, c) => { need(args, 2); const test = criteria(scalarOf(run(args[1], c))); return [...range(args[0], c).values()].filter(test).length; },
  COUNTIFS: (args, c) => { const pairs = ifs(args, c, 0); const size = pairs[0].where.height * pairs[0].where.width; let count = 0; for (let i = 0; i < size; i++) if (matching(pairs, i)) count++; return count; },
  AVERAGEIF: (args, c) => {
    const where = range(args[0], c), test = criteria(scalarOf(run(args[1], c))), avg = args[2] ? range(args[2], c) : where;
    const list = [];
    where.rows.forEach((row, r) => row.forEach((value, col) => { const add = avg.rows[r]?.[col]; if (test(value) && typeof add === 'number') list.push(add); }));
    if (!list.length) fail('#DIV/0!');
    return list.reduce((a, b) => a + b, 0) / list.length;
  },
  VLOOKUP: (args, c) => {
    need(args, 3, 4);
    const target = scalar(run(args[0], c)), table = range(args[1], c), col = Math.trunc(argNum(args, 2, c));
    if (col < 1 || col > table.width) fail('#REF!');
    const exact = args[3] && args[3].k !== 'missing' ? !bool(run(args[3], c)) : false;
    const index = lookupIndex(table.rows.map(row => row[0]), target, exact ? 0 : 1);
    if (index < 0) fail('#N/A');
    return table.rows[index][col - 1];
  },
  HLOOKUP: (args, c) => {
    need(args, 3, 4);
    const target = scalar(run(args[0], c)), table = range(args[1], c), row = Math.trunc(argNum(args, 2, c));
    if (row < 1 || row > table.height) fail('#REF!');
    const exact = args[3] && args[3].k !== 'missing' ? !bool(run(args[3], c)) : false;
    const index = lookupIndex(table.rows[0], target, exact ? 0 : 1);
    if (index < 0) fail('#N/A');
    return table.rows[row - 1][index];
  },
  MATCH: (args, c) => {
    need(args, 2, 3);
    const target = scalar(run(args[0], c)), list = [...range(args[1], c).values()];
    const index = lookupIndex(list, target, Math.sign(argNum(args, 2, c, 1)));
    if (index < 0) fail('#N/A');
    return index + 1;
  },
  INDEX: (args, c) => {
    need(args, 2, 3);
    const table = range(args[0], c);
    let row = Math.trunc(argNum(args, 1, c, 0)), col = Math.trunc(argNum(args, 2, c, 0));
    if (table.height === 1 && args.length === 2) { col = row; row = 1; }
    if (!col && table.width === 1) col = 1;
    if (!row && table.height === 1) row = 1;
    if (row < 1 || col < 1 || row > table.height || col > table.width) fail('#REF!');
    return table.rows[row - 1][col - 1];
  },
  XLOOKUP: (args, c) => {
    need(args, 3, 6);
    const target = scalar(run(args[0], c)), look = [...range(args[1], c).values()], back = range(args[2], c);
    const index = lookupIndex(look, target, 0);
    if (index < 0) {
      if (args[3] && args[3].k !== 'missing') return run(args[3], c);
      fail('#N/A');
    }
    return back.height === 1 ? back.rows[0][index] : back.rows[index][0];
  },
};

/** The functions above with their arguments and what they do, for suggestions while typing. */
export const FUNCTION_HELP = {
  SUM: ['number1, [number2], …', 'Adds numbers'],
  PRODUCT: ['number1, [number2], …', 'Multiplies numbers'],
  AVERAGE: ['number1, [number2], …', 'The average of numbers'],
  AVERAGEA: ['value1, [value2], …', 'The average, counting text as 0 and TRUE as 1'],
  MIN: ['number1, [number2], …', 'The smallest number'],
  MAX: ['number1, [number2], …', 'The largest number'],
  MEDIAN: ['number1, [number2], …', 'The middle number'],
  LARGE: ['array, k', 'The k-th largest number'],
  SMALL: ['array, k', 'The k-th smallest number'],
  COUNT: ['value1, [value2], …', 'How many numbers'],
  COUNTA: ['value1, [value2], …', 'How many cells are not empty'],
  COUNTBLANK: ['range', 'How many cells are empty'],
  ABS: ['number', 'The number without its sign'],
  INT: ['number', 'Rounds down to a whole number'],
  TRUNC: ['number, [digits]', 'Cuts off decimals'],
  ROUND: ['number, digits', 'Rounds to a number of digits'],
  ROUNDUP: ['number, digits', 'Rounds away from zero'],
  ROUNDDOWN: ['number, digits', 'Rounds towards zero'],
  MOD: ['number, divisor', 'The remainder of a division'],
  POWER: ['number, power', 'A number raised to a power'],
  SQRT: ['number', 'The square root'],
  SIGN: ['number', '1, 0 or -1, by the sign'],
  PI: ['', 'The number π'],
  CEILING: ['number, [significance]', 'Rounds up to a multiple'],
  FLOOR: ['number, [significance]', 'Rounds down to a multiple'],
  IF: ['test, value_if_true, [value_if_false]', 'One value if a test is true, another if not'],
  IFERROR: ['value, value_if_error', 'A value, or another if it is an error'],
  IFNA: ['value, value_if_na', 'A value, or another if it is #N/A'],
  AND: ['test1, [test2], …', 'TRUE if all are true'],
  OR: ['test1, [test2], …', 'TRUE if any is true'],
  XOR: ['test1, [test2], …', 'TRUE if an odd number are true'],
  NOT: ['test', 'The opposite of a test'],
  TRUE: ['', 'The value TRUE'],
  FALSE: ['', 'The value FALSE'],
  NA: ['', 'The error #N/A'],
  CHOOSE: ['index, value1, [value2], …', 'The value at a position in the list'],
  CONCATENATE: ['text1, [text2], …', 'Joins texts'],
  CONCAT: ['text1, [text2], …', 'Joins texts and ranges'],
  TEXTJOIN: ['separator, skip_empty, text1, [text2], …', 'Joins texts with a separator'],
  LEN: ['text', 'The number of characters'],
  LEFT: ['text, [count]', 'Characters from the start'],
  RIGHT: ['text, [count]', 'Characters from the end'],
  MID: ['text, start, count', 'Characters from the middle'],
  UPPER: ['text', 'Text in capitals'],
  LOWER: ['text', 'Text in small letters'],
  PROPER: ['text', 'Text With Each Word Capitalized'],
  TRIM: ['text', 'Text without extra spaces'],
  REPT: ['text, times', 'Text repeated'],
  EXACT: ['text1, text2', 'TRUE if two texts are the same, case included'],
  SUBSTITUTE: ['text, old, new, [which]', 'Replaces text with other text'],
  FIND: ['find, within, [start]', 'Where text is found, case counted'],
  SEARCH: ['find, within, [start]', 'Where text is found, case ignored'],
  VALUE: ['text', 'Text as a number'],
  TEXT: ['value, format', 'A value as text in a number format'],
  ISBLANK: ['value', 'TRUE if empty'],
  ISNUMBER: ['value', 'TRUE if a number'],
  ISTEXT: ['value', 'TRUE if text'],
  ISLOGICAL: ['value', 'TRUE if TRUE or FALSE'],
  ISERROR: ['value', 'TRUE if an error'],
  ISNA: ['value', 'TRUE if #N/A'],
  DATE: ['year, month, day', 'A date'],
  YEAR: ['date', 'The year of a date'],
  MONTH: ['date', 'The month of a date'],
  DAY: ['date', 'The day of a date'],
  HOUR: ['time', 'The hour of a time'],
  MINUTE: ['time', 'The minutes of a time'],
  SECOND: ['time', 'The seconds of a time'],
  WEEKDAY: ['date, [type]', 'The day of the week, as a number'],
  ROW: ['[reference]', 'The row number'],
  COLUMN: ['[reference]', 'The column number'],
  SUMIF: ['range, criteria, [sum_range]', 'Adds the cells that match'],
  SUMIFS: ['sum_range, range1, criteria1, …', 'Adds the cells that match every test'],
  COUNTIF: ['range, criteria', 'Counts the cells that match'],
  COUNTIFS: ['range1, criteria1, …', 'Counts the cells that match every test'],
  AVERAGEIF: ['range, criteria, [average_range]', 'The average of the cells that match'],
  VLOOKUP: ['value, table, column, [approximate]', 'Looks down the first column, returns from another'],
  HLOOKUP: ['value, table, row, [approximate]', 'Looks along the first row, returns from another'],
  MATCH: ['value, range, [type]', 'The position of a value in a range'],
  INDEX: ['range, row, [column]', 'The value at a row and column of a range'],
  XLOOKUP: ['value, lookup_range, return_range, [if_not_found]', 'Finds a value, returns from another range'],
};

// --- Typing a formula ---------------------------------------------------------------------------

/** `text` before `caret` with what is inside quotes blanked out, or null when it is not a formula. */
function formulaBefore(text, caret) {
  if (!text.startsWith('=')) return null;
  let out = '', quote = '';
  for (const char of text.slice(0, caret)) {
    if (quote) { if (char === quote) quote = ''; out += ' '; continue; }
    if (char === '"' || char === "'") { quote = char; out += ' '; continue; }
    out += char;
  }
  return quote ? null : out;
}

/** The function name being typed just before `caret`, as { start, word }, or null. */
export function formulaWordAt(text, caret) {
  const before = formulaBefore(text, caret);
  const match = before && /(^=|[=(,;+\-*/^&<>:\s])([A-Za-z][A-Za-z0-9.]*)$/.exec(before);
  return match ? { start: caret - match[2].length, word: match[2] } : null;
}

/** The function call `caret` is inside, as { name, arg } (arg counted from 0), or null. */
export function callAt(text, caret) {
  const before = formulaBefore(text, caret);
  if (!before) return null;
  let depth = 0, arg = 0;
  for (let i = before.length - 1; i > 0; i--) {
    const char = before[i];
    if (char === ')') depth++;
    else if (char === '(') {
      if (depth) { depth--; continue; }
      const name = /([A-Za-z][A-Za-z0-9.]*)$/.exec(before.slice(0, i));
      return name ? { name: name[1].toUpperCase(), arg } : null;
    } else if ((char === ',' || char === ';') && !depth) arg++;
  }
  return null;
}

/** Whether a reference can go at `caret`: after `=`, an operator, `(` or a separator. */
export function canPointAt(text, caret) {
  const before = formulaBefore(text, caret);
  return !!before && /[=(,;+\-*/^&<>:]\s*$/.test(before);
}

// --- Dates --------------------------------------------------------------------------------------

const DAY_MS = 86400000;
const EPOCH_1900 = Date.UTC(1899, 11, 30);
const EPOCH_1904 = Date.UTC(1904, 0, 1);

/** A date serial's parts, read in UTC so local time zones never shift a day. */
export function serialParts(serial, date1904 = false) {
  if (!Number.isFinite(serial) || serial < 0 || serial > 2958465.99999) fail('#NUM!');
  const whole = Math.floor(serial);
  let seconds = Math.round((serial - whole) * 86400);
  let days = whole;
  if (seconds >= 86400) { days += 1; seconds -= 86400; }
  if (!date1904 && whole === 60) return { year: 1900, month: 2, day: 29, hours: Math.floor(seconds / 3600), minutes: Math.floor(seconds / 60) % 60, seconds: seconds % 60, weekday: 3 };
  const base = date1904 ? EPOCH_1904 : EPOCH_1900;
  const date = new Date(base + (days + (!date1904 && whole < 60 ? 1 : 0)) * DAY_MS);
  return {
    year: date.getUTCFullYear(), month: date.getUTCMonth() + 1, day: date.getUTCDate(),
    hours: Math.floor(seconds / 3600), minutes: Math.floor(seconds / 60) % 60, seconds: seconds % 60,
    weekday: date.getUTCDay(),
  };
}

export function dateSerial(year, month, day, date1904 = false) {
  if (year >= 0 && year < 1900) year += 1900;
  const ms = Date.UTC(Math.trunc(year), Math.trunc(month) - 1, Math.trunc(day));
  if (date1904) return (ms - EPOCH_1904) / DAY_MS;
  const serial = (ms - EPOCH_1900) / DAY_MS;
  return serial < 61 ? serial - 1 : serial;
}

// --- Number formats -----------------------------------------------------------------------------

export const BUILTIN_FORMATS = {
  0: 'General', 1: '0', 2: '0.00', 3: '#,##0', 4: '#,##0.00', 9: '0%', 10: '0.00%', 11: '0.00E+00',
  12: '# ?/?', 13: '# ??/??', 14: 'yyyy-mm-dd', 15: 'd-mmm-yy', 16: 'd-mmm', 17: 'mmm-yy', 18: 'h:mm AM/PM',
  19: 'h:mm:ss AM/PM', 20: 'h:mm', 21: 'h:mm:ss', 22: 'yyyy-mm-dd h:mm', 37: '#,##0 ;(#,##0)',
  38: '#,##0 ;[Red](#,##0)', 39: '#,##0.00;(#,##0.00)', 40: '#,##0.00;[Red](#,##0.00)', 45: 'mm:ss',
  46: '[h]:mm:ss', 47: 'mm:ss.0', 48: '##0.0E+0', 49: '@',
};

const COLORS = { black: '#000000', blue: '#0000ff', cyan: '#00ffff', green: '#008000', magenta: '#ff00ff', red: '#ff0000', white: '#ffffff', yellow: '#ffff00' };

function splitSections(code) {
  const sections = [];
  let current = '', quoted = false;
  for (let i = 0; i < code.length; i++) {
    const char = code[i];
    if (char === '"') quoted = !quoted;
    if (!quoted && char === '\\') { current += char + (code[i + 1] ?? ''); i++; continue; }
    if (!quoted && char === ';') { sections.push(current); current = ''; continue; }
    current += char;
  }
  sections.push(current);
  return sections;
}

function stripBrackets(section) {
  let color = null, condition = null, elapsed = false;
  const body = section.replace(/\[([^\]]*)\]/g, (all, inner) => {
    if (COLORS[inner.toLowerCase()]) { color = COLORS[inner.toLowerCase()]; return ''; }
    if (/^color\s*\d+$/i.test(inner)) return '';
    const cond = /^(<=|>=|<>|<|>|=)\s*(-?[\d.]+)$/.exec(inner);
    if (cond) { condition = { op: cond[1], value: Number(cond[2]) }; return ''; }
    if (/^\$/.test(inner)) { const symbol = inner.slice(1).split('-')[0]; return symbol ? `"${symbol.replace(/"/g, '')}"` : ''; }
    if (/^(h+|m+|s+)$/i.test(inner)) { elapsed = true; return `[${inner.toLowerCase()}]`; }
    return '';
  });
  return { body, color, condition, elapsed };
}

const DATE_TOKENS = /(?:"[^"]*"|\\.|_.|\*.|\[(?:h+|m+|s+)\]|[yY]+|[dD]+|[hH]+|[sS]+|[mM]+|AM\/PM|am\/pm|A\/P|a\/p|[eE][-+])/g;
function isDateFormat(body) {
  const tokens = body.match(DATE_TOKENS) || [];
  return tokens.some(token => /^(\[(h+|m+|s+)\]|[yY]+|[dD]+|[hH]+|[sS]+|[mM]+)$/.test(token)) && !tokens.some(token => /^[eE][-+]$/.test(token));
}

/** Locale separators for showing numbers, read once from the browser. */
export function localeSeparators(locale) {
  try {
    const parts = new Intl.NumberFormat(locale).formatToParts(12345.6);
    return { decimal: parts.find(p => p.type === 'decimal')?.value || '.', group: parts.find(p => p.type === 'group')?.value || ',' };
  } catch {
    return { decimal: '.', group: ',' };
  }
}

export function formatGeneral(value, { decimal = '.' } = {}) {
  if (value === 0) return '0';
  const abs = Math.abs(value);
  let out;
  if (abs >= 1e11 || abs < 1e-9) {
    out = value.toExponential(5).replace(/\.?0+e/, 'e').replace('e', 'E').replace(/E([-+])(\d)$/, 'E$10$2');
  } else {
    const digits = Math.max(1, 10 - Math.max(0, Math.floor(Math.log10(abs))));
    out = String(Number(value.toFixed(Math.min(digits, 10))));
    if (/e/i.test(out)) out = value.toFixed(Math.min(digits, 10)).replace(/\.?0+$/, '');
  }
  return decimal === '.' ? out : out.replace('.', decimal);
}

/**
 * Formats a cell value with an Excel number format code.
 * Returns the text and, when the code asks for one, a colour.
 */
export function formatValue(value, code = 'General', options = {}) {
  const { decimal = '.', group = ',', date1904 = false } = options;
  if (value == null || value === '') return { text: '' };
  if (value instanceof CellError) return { text: value.code };
  if (typeof value === 'boolean') return { text: value ? 'TRUE' : 'FALSE' };
  const sections = splitSections(code || 'General');
  if (typeof value === 'string') {
    const section = sections.length >= 4 ? sections[3] : sections.find(s => s.includes('@'));
    if (!section) return { text: value };
    const { body, color } = stripBrackets(section);
    return { text: literalText(body, value), color };
  }
  if (!Number.isFinite(value)) return { text: '#NUM!' };
  let index = 0, absolute = false;
  const parsed = sections.map(stripBrackets);
  if (parsed.some(section => section.condition)) {
    index = parsed.findIndex(section => !section.condition || test(section.condition, value));
    if (index < 0) index = parsed.length - 1;
    absolute = index > 0 && value < 0 && !parsed[index].condition;
  } else if (value < 0 && parsed.length >= 2) { index = 1; absolute = true; }
  else if (value === 0 && parsed.length >= 3) index = 2;
  const section = parsed[index] || parsed[0];
  const number = absolute ? Math.abs(value) : value;
  let text;
  if (/^\s*general\s*$/i.test(section.body.replace(/"[^"]*"/g, ''))) text = section.body.replace(/general/i, formatGeneral(number, { decimal })).replace(/"([^"]*)"/g, '$1');
  else if (/general/i.test(section.body)) text = literalText(section.body.replace(/general/i, '@'), formatGeneral(number, { decimal }));
  else if (isDateFormat(section.body)) {
    try { text = formatDate(number, section.body, date1904); } catch { text = '#'.repeat(8); }
  } else text = formatNumber(number, section.body, { decimal, group });
  return { text, color: section.color };
}

function test(condition, value) {
  const { op, value: limit } = condition;
  return { '<': value < limit, '>': value > limit, '<=': value <= limit, '>=': value >= limit, '=': value === limit, '<>': value !== limit }[op];
}

function literalText(body, value) {
  let out = '';
  for (let i = 0; i < body.length; i++) {
    const char = body[i];
    if (char === '"') { const end = body.indexOf('"', i + 1); out += body.slice(i + 1, end < 0 ? undefined : end); i = end < 0 ? body.length : end; }
    else if (char === '\\') out += body[++i] ?? '';
    else if (char === '_') { out += ' '; i++; }
    else if (char === '*') i++;
    else if (char === '@') out += value;
    else out += char;
  }
  return out;
}

function formatNumber(value, body, { decimal, group }) {
  // Literals stay where they are; digit placeholders form the number.
  const items = [];
  for (let i = 0; i < body.length; i++) {
    const char = body[i];
    if (char === '"') { const end = body.indexOf('"', i + 1); items.push({ lit: body.slice(i + 1, end < 0 ? undefined : end) }); i = end < 0 ? body.length : end; }
    else if (char === '\\') items.push({ lit: body[++i] ?? '' });
    else if (char === '_') { items.push({ lit: ' ' }); i++; }
    else if (char === '*') i++;
    else if ('0#?'.includes(char)) items.push({ digit: char });
    else if (char === '.') items.push({ dot: true });
    else if (char === ',') items.push({ comma: true });
    else if (char === '%') items.push({ percent: true, lit: '%' });
    else if ((char === 'E' || char === 'e') && /[-+]/.test(body[i + 1] || '')) { items.push({ exp: body[i + 1] }); i++; }
    else if (char === '/') return formatGeneral(value, { decimal });
    else if (char === '@') items.push({ lit: formatGeneral(value, { decimal }) });
    else items.push({ lit: char });
  }
  const percents = items.filter(item => item.percent).length;
  value *= Math.pow(100, percents);
  const expAt = items.findIndex(item => item.exp);
  const mantissaItems = expAt < 0 ? items : items.slice(0, expAt);
  const dotAt = mantissaItems.findIndex(item => item.dot);
  const intItems = dotAt < 0 ? mantissaItems : mantissaItems.slice(0, dotAt);
  const fracItems = dotAt < 0 ? [] : mantissaItems.slice(dotAt + 1);
  const intDigits = intItems.filter(item => item.digit);
  const fracDigits = fracItems.filter(item => item.digit);
  if (!intDigits.length && !fracDigits.length) return literalText(body, '');
  const firstDigit = intItems.findIndex(item => item.digit), lastDigit = intItems.length - 1 - [...intItems].reverse().findIndex(item => item.digit);
  const grouping = intDigits.length > 0 && intItems.some((item, i) => item.comma && i > firstDigit && i < lastDigit);
  let exponent = 0, mantissa = Math.abs(value);
  if (expAt >= 0 && mantissa !== 0) {
    const intCount = Math.max(1, intDigits.length);
    exponent = Math.floor(Math.log10(mantissa)) - (intCount - 1);
    mantissa /= Math.pow(10, exponent);
  }
  const places = fracDigits.length;
  let fixed = (Math.round(Number((mantissa * Math.pow(10, places)).toPrecision(15))) / Math.pow(10, places)).toFixed(places);
  if (expAt >= 0 && Number(fixed) >= Math.pow(10, Math.max(1, intDigits.length))) {
    exponent += 1;
    mantissa /= 10;
    fixed = mantissa.toFixed(places);
  }
  let [whole, fraction = ''] = fixed.split('.');
  if (whole === '0' && !intDigits.some(item => item.digit === '0')) whole = '';
  const required = intDigits.filter(item => item.digit === '0').length;
  if (whole.length < required) whole = whole.padStart(required, '0');
  if (grouping) whole = whole.replace(/\B(?=(\d{3})+(?!\d))/g, '\u0000');
  // Trailing optional decimals: '#' drops a zero, '?' leaves a space.
  const fracOut = fraction.split('');
  for (let i = fracDigits.length - 1; i >= 0; i--) {
    if (fracOut[i] !== '0' || fracDigits[i].digit === '0') break;
    fracOut[i] = fracDigits[i].digit === '?' ? ' ' : '';
  }
  // Integer placeholders fill from the right; the leftmost takes any extra digits.
  const intOut = new Array(intItems.length).fill('');
  let rest = whole;
  for (let i = intItems.length - 1; i >= 0; i--) {
    const item = intItems[i];
    if (!item.digit) continue;
    let take = '';
    if (i === firstDigit) { take = rest; rest = ''; }
    else if (rest) {
      take = rest.slice(-1); rest = rest.slice(0, -1);
      if (take === '\u0000' && rest) { take = rest.slice(-1) + take; rest = rest.slice(0, -1); }
    }
    intOut[i] = take || (item.digit === '?' ? ' ' : '');
  }
  let out = '';
  intItems.forEach((item, i) => { if (item.digit) out += intOut[i]; else if (item.lit != null) out += item.lit; });
  if (dotAt >= 0) {
    const anyFraction = fracOut.some(char => char !== '');
    if (anyFraction || fracDigits.every(item => item.digit !== '#')) out += decimal;
    let f = 0;
    for (const item of fracItems) {
      if (item.digit) out += fracOut[f++] ?? '';
      else if (item.lit != null) out += item.lit;
    }
  }
  if (expAt >= 0) {
    const expItems = items.slice(expAt + 1);
    const width = expItems.filter(item => item.digit === '0').length || 1;
    out += 'E' + (exponent < 0 ? '-' : items[expAt].exp === '+' ? '+' : '') + String(Math.abs(exponent)).padStart(width, '0');
    for (const item of expItems) if (!item.digit && item.lit != null) out += item.lit;
  }
  out = out.replace(/\u0000/g, group);
  const negative = value < 0 && Number(fixed) !== 0;
  return negative ? `-${out}` : out;
}

const nameCache = new Map();
function names(kind, locale) {
  const key = `${kind}:${locale}`;
  if (!nameCache.has(key)) {
    const list = [];
    for (let i = 0; i < (kind.startsWith('month') ? 12 : 7); i++) {
      const date = kind.startsWith('month') ? new Date(Date.UTC(2021, i, 1)) : new Date(Date.UTC(2021, 0, 3 + i));
      const style = kind.endsWith('long') ? 'long' : 'short';
      let text;
      try { text = new Intl.DateTimeFormat(locale, { [kind.startsWith('month') ? 'month' : 'weekday']: style, timeZone: 'UTC' }).format(date); } catch { text = ''; }
      list.push(text.replace(/\.$/, ''));
    }
    nameCache.set(key, list);
  }
  return nameCache.get(key);
}

function formatDate(serial, body, date1904) {
  const parts = serialParts(serial, date1904);
  const locale = typeof navigator !== 'undefined' ? navigator.language : 'en-US';
  const tokens = [];
  const re = /"[^"]*"|\\.|_.|\*.|\[(h+|m+|s+)\]|y+|d+|h+|m+|s+(?:\.0+)?|AM\/PM|am\/pm|A\/P|a\/p|./gi;
  let match;
  while ((match = re.exec(body))) tokens.push(match[0]);
  const twelve = tokens.some(token => /^(AM\/PM|A\/P)$/i.test(token));
  // An "m" right after hours or before seconds means minutes.
  const kinds = tokens.map(token => (/^m+$/i.test(token) ? 'm' : /^h+$/i.test(token) || /^\[h+\]$/i.test(token) ? 'h' : /^s+/i.test(token) || /^\[s+\]$/i.test(token) ? 's' : /^[yd]+$/i.test(token) ? 'date' : null));
  const minuteAt = new Set();
  kinds.forEach((kind, i) => {
    if (kind !== 'm') return;
    const before = kinds.slice(0, i).reverse().find(Boolean), after = kinds.slice(i + 1).find(Boolean);
    if (before === 'h' || after === 's') minuteAt.add(i);
  });
  const total = serial * 86400;
  return tokens.map((token, i) => {
    const lower = token.toLowerCase();
    if (token.startsWith('"')) return token.slice(1, -1);
    if (token.startsWith('\\')) return token.slice(1);
    if (token.startsWith('_')) return ' ';
    if (token.startsWith('*')) return '';
    if (/^\[h+\]$/.test(lower)) return String(Math.floor(total / 3600)).padStart(lower.length - 2, '0');
    if (/^\[m+\]$/.test(lower)) return String(Math.floor(total / 60)).padStart(lower.length - 2, '0');
    if (/^\[s+\]$/.test(lower)) return String(Math.round(total)).padStart(lower.length - 2, '0');
    if (/^y+$/.test(lower)) return lower.length <= 2 ? String(parts.year % 100).padStart(2, '0') : String(parts.year);
    if (/^d+$/.test(lower)) {
      if (lower.length === 1) return String(parts.day);
      if (lower.length === 2) return String(parts.day).padStart(2, '0');
      return names(lower.length === 3 ? 'day-short' : 'day-long', locale)[parts.weekday];
    }
    if (/^m+$/.test(lower)) {
      if (minuteAt.has(i)) return lower.length >= 2 ? String(parts.minutes).padStart(2, '0') : String(parts.minutes);
      if (lower.length === 1) return String(parts.month);
      if (lower.length === 2) return String(parts.month).padStart(2, '0');
      if (lower.length === 5) return names('month-long', locale)[parts.month - 1].charAt(0);
      return names(lower.length === 3 ? 'month-short' : 'month-long', locale)[parts.month - 1];
    }
    if (/^h+$/.test(lower)) {
      let hours = parts.hours;
      if (twelve) hours = hours % 12 || 12;
      return lower.length >= 2 ? String(hours).padStart(2, '0') : String(hours);
    }
    if (/^s+/.test(lower)) {
      const [secs, frac] = lower.split('.');
      const fraction = serial * 86400 - Math.floor(serial * 86400);
      const out = secs.length >= 2 ? String(parts.seconds).padStart(2, '0') : String(parts.seconds);
      return frac ? out + '.' + String(Math.floor(fraction * Math.pow(10, frac.length))).padStart(frac.length, '0') : out;
    }
    if (/^am\/pm$/i.test(token)) return parts.hours < 12 ? 'AM' : 'PM';
    if (/^a\/p$/i.test(token)) return parts.hours < 12 ? 'A' : 'P';
    return token;
  }).join('');
}
