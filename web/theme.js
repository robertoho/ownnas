// The saved theme, set before the page draws so it does not flash the default one. (A file, not
// an inline script: the page's security policy runs only its own files.) Keep the lists in step
// with THEMES in app.js.
(() => {
  const light = ["catppuccin-latte", "rose-pine", "liquid-glass", "windows-11", "github-light", "one-light", "solarized-light", "gruvbox-light", "everforest-light", "tokyo-night-day", "kanagawa-lotus", "nord-light", "flexoki-light", "paper"];
  const dark = ["ownnas", "tokyo-night", "catppuccin", "everforest", "gruvbox", "kanagawa", "osaka-jade", "matte-black", "nord", "dracula", "solarized-dark"];
  let theme = null;
  try {
    theme = localStorage.getItem("ownnas-theme");
  } catch {}
  if (!light.includes(theme) && !dark.includes(theme)) theme = "ownnas";
  document.documentElement.setAttribute("data-theme", theme);
  document.documentElement.setAttribute("data-mode", light.includes(theme) ? "light" : "dark");
})();
