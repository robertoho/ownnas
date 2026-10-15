// The saved theme, set before the page draws so it does not flash the default one. (A file, not
// an inline script: the page's security policy runs only its own files.)
(() => {
  const allowed = ["ownnas", "tokyo-night", "catppuccin", "catppuccin-latte", "everforest", "gruvbox", "kanagawa", "osaka-jade", "matte-black", "rose-pine", "liquid-glass", "windows-11"];
  let theme = null;
  try {
    theme = localStorage.getItem("ownnas-theme");
  } catch {}
  document.documentElement.setAttribute("data-theme", allowed.includes(theme) ? theme : "ownnas");
})();
