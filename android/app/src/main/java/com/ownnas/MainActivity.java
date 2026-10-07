package com.ownnas;

import android.app.Activity;
import android.content.Intent;
import android.graphics.Color;
import android.net.Uri;
import android.os.Bundle;
import android.os.Environment;
import android.view.View;
import android.view.ViewGroup;
import android.webkit.ValueCallback;
import android.webkit.WebChromeClient;
import android.webkit.WebResourceRequest;
import android.webkit.WebSettings;
import android.webkit.WebView;
import android.webkit.WebViewClient;
import android.widget.Button;
import android.widget.EditText;
import android.widget.LinearLayout;
import android.widget.ScrollView;
import android.widget.TextView;
import android.widget.Toast;

import java.io.File;

public class MainActivity extends Activity {
    static {
        System.loadLibrary("ownnas");
    }

    private static final int FILE_CHOOSER_REQUEST = 1001;
    private ValueCallback<Uri[]> filePathCallback;
    private WebView webView;
    private EditText rootInput;
    private EditText usernameInput;
    private EditText passwordInput;

    private static native int startOwnnas(String root, String data, String username, String password);
    private static native void stopOwnnas();

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        showSetup();
    }

    private void showSetup() {
        ScrollView scroll = new ScrollView(this);
        LinearLayout content = new LinearLayout(this);
        content.setOrientation(LinearLayout.VERTICAL);
        content.setPadding(dp(24), dp(32), dp(24), dp(24));
        scroll.addView(content);

        TextView title = new TextView(this);
        title.setText("OwnNAS");
        title.setTextColor(Color.rgb(28, 35, 48));
        title.setTextSize(32);
        title.setTypeface(null, 1);
        content.addView(title, params(1));

        TextView subtitle = new TextView(this);
        subtitle.setText("Rust server + the existing OwnNAS web interface");
        subtitle.setTextColor(Color.DKGRAY);
        subtitle.setTextSize(16);
        content.addView(subtitle, params(0, 0, 0, 24));

        rootInput = field("Folder to share");
        rootInput.setText(defaultRoot().getAbsolutePath());
        content.addView(rootInput, params(0, 0, 0, 12));

        TextView hint = new TextView(this);
        hint.setText("The default folder is private to this app. Upload files from the OwnNAS page, or enter a path that Android grants this app access to.");
        hint.setTextColor(Color.GRAY);
        hint.setTextSize(13);
        content.addView(hint, params(0, 0, 0, 20));

        usernameInput = field("Username");
        usernameInput.setText("admin");
        content.addView(usernameInput, params(0, 0, 0, 12));

        passwordInput = field("Password");
        passwordInput.setInputType(0x00000081); // TYPE_CLASS_TEXT | TYPE_TEXT_VARIATION_PASSWORD
        content.addView(passwordInput, params(0, 0, 0, 20));

        Button start = new Button(this);
        start.setText("Start OwnNAS");
        start.setOnClickListener(view -> startServer());
        content.addView(start, params(0));

        TextView footer = new TextView(this);
        footer.setText("The server listens on this device only (127.0.0.1).");
        footer.setTextColor(Color.GRAY);
        footer.setTextSize(13);
        content.addView(footer, params(0, 24, 0, 0));

        setContentView(scroll);
    }

    private void startServer() {
        String root = rootInput.getText().toString().trim();
        String username = usernameInput.getText().toString().trim();
        String password = passwordInput.getText().toString();
        if (root.isEmpty() || username.isEmpty() || password.isEmpty()) {
            Toast.makeText(this, "Enter a folder, username, and password", Toast.LENGTH_LONG).show();
            return;
        }

        try {
            File rootDir = new File(root);
            if (!rootDir.exists() && !rootDir.mkdirs()) {
                throw new IllegalStateException("Could not create the shared folder");
            }
            File dataDir = new File(getFilesDir(), "ownnas-data");
            int port = startOwnnas(root, dataDir.getAbsolutePath(), username, password);
            showBrowser(port);
        } catch (RuntimeException error) {
            Toast.makeText(this, error.getMessage(), Toast.LENGTH_LONG).show();
        }
    }

    private void showBrowser(int port) {
        webView = new WebView(this);
        WebSettings settings = webView.getSettings();
        settings.setJavaScriptEnabled(true);
        settings.setDomStorageEnabled(true);
        settings.setAllowFileAccess(false);
        settings.setAllowContentAccess(true);
        settings.setMediaPlaybackRequiresUserGesture(false);
        webView.setWebViewClient(new WebViewClient() {
            @Override
            public boolean shouldOverrideUrlLoading(WebView view, WebResourceRequest request) {
                return false;
            }
        });
        webView.setWebChromeClient(new WebChromeClient() {
            @Override
            public boolean onShowFileChooser(WebView view, ValueCallback<Uri[]> callback, FileChooserParams params) {
                if (filePathCallback != null) {
                    filePathCallback.onReceiveValue(null);
                }
                filePathCallback = callback;
                Intent intent = new Intent(Intent.ACTION_OPEN_DOCUMENT);
                intent.addCategory(Intent.CATEGORY_OPENABLE);
                intent.setType("*/*");
                intent.putExtra(Intent.EXTRA_ALLOW_MULTIPLE, true);
                startActivityForResult(intent, FILE_CHOOSER_REQUEST);
                return true;
            }
        });
        setContentView(webView);
        webView.loadUrl("http://127.0.0.1:" + port + "/");
    }

    private File defaultRoot() {
        File documents = getExternalFilesDir(Environment.DIRECTORY_DOCUMENTS);
        File root = new File(documents == null ? getFilesDir() : documents, "OwnNAS");
        if (!root.exists()) {
            //noinspection ResultOfMethodCallIgnored
            root.mkdirs();
        }
        return root;
    }

    @Override
    protected void onActivityResult(int requestCode, int resultCode, Intent data) {
        super.onActivityResult(requestCode, resultCode, data);
        if (requestCode != FILE_CHOOSER_REQUEST || filePathCallback == null) {
            return;
        }
        Uri[] results = null;
        if (resultCode == RESULT_OK && data != null) {
            if (data.getClipData() != null) {
                int count = data.getClipData().getItemCount();
                results = new Uri[count];
                for (int index = 0; index < count; index++) {
                    results[index] = data.getClipData().getItemAt(index).getUri();
                }
            } else if (data.getData() != null) {
                results = new Uri[] { data.getData() };
            }
        }
        filePathCallback.onReceiveValue(results);
        filePathCallback = null;
    }

    @Override
    public void onBackPressed() {
        if (webView != null && webView.canGoBack()) {
            webView.goBack();
        } else {
            super.onBackPressed();
        }
    }

    @Override
    protected void onDestroy() {
        if (isFinishing()) {
            stopOwnnas();
        }
        super.onDestroy();
    }

    private EditText field(String hint) {
        EditText field = new EditText(this);
        field.setHint(hint);
        field.setTextSize(16);
        field.setSingleLine(true);
        return field;
    }

    private LinearLayout.LayoutParams params(int bottom) {
        return params(0, 0, 0, bottom);
    }

    private LinearLayout.LayoutParams params(int left, int top, int right, int bottom) {
        LinearLayout.LayoutParams value = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.WRAP_CONTENT);
        value.setMargins(dp(left), dp(top), dp(right), dp(bottom));
        return value;
    }

    private int dp(int value) {
        return (int) (value * getResources().getDisplayMetrics().density + 0.5f);
    }
}
