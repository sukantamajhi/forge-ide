// Forge landing page: the editor illustration (typing, theme switch), a filterable command palette,
// a commit-message demo, and the early-access form.

// Set this to the URL that accepts a POST of {"email": "..."} (Formspree, Basin, your own server).
// While it is empty the form is switched off and says so, rather than pretending to save an address.
const WAITLIST_ENDPOINT = "";

const reduceMotion = window.matchMedia("(prefers-reduced-motion: reduce)").matches;

(function typeLine() {
  const target = document.getElementById("typed");
  if (!target || reduceMotion) return;
  const text = target.textContent;
  target.textContent = "";
  let i = 0;
  const step = () => {
    i += 1;
    target.textContent = text.slice(0, i);
    if (i < text.length) setTimeout(step, 55 + Math.random() * 70);
  };
  setTimeout(step, 1100);
})();

(function themes() {
  const win = document.querySelector(".win");
  const buttons = document.querySelectorAll(".seg button");
  buttons.forEach((button) => {
    button.addEventListener("click", () => {
      win.dataset.theme = button.dataset.theme;
      buttons.forEach((b) => b.setAttribute("aria-pressed", String(b === button)));
    });
  });
})();

(function palette() {
  const commands = [
    ["Go to File...", "Ctrl+P"],
    ["Go to Line...", "Ctrl+G"],
    ["Go to Symbol in File...", "Ctrl+Shift+O"],
    ["Find in Files", "Ctrl+Shift+F"],
    ["Replace in Files", "Ctrl+Shift+H"],
    ["Split Editor", "Ctrl+\\"],
    ["Format Document", "Shift+Alt+F"],
    ["Markdown: Open Preview to the Side", "Ctrl+K V"],
    ["Git: Generate Commit Message with AI", ""],
    ["Git: View File History", ""],
    ["Debug: Install Rust Debugger (CodeLLDB)", ""],
    ["Extensions: Install from Open VSX...", ""],
    ["Extensions: Restart Extension Host", ""],
    ["Language Servers: Manage and Install...", ""],
    ["Preferences: Color Theme", ""],
    ["Toggle Minimap", ""],
  ];
  const input = document.getElementById("pal-q");
  const list = document.getElementById("pal-list");
  const empty = document.getElementById("pal-empty");
  if (!input || !list) return;
  let selected = 0;
  let shown = [];

  const escapeHtml = (s) => s.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c]));
  const render = () => {
    const q = input.value.trim().toLowerCase();
    shown = commands.filter(([name]) => name.toLowerCase().includes(q));
    selected = Math.min(selected, Math.max(shown.length - 1, 0));
    list.innerHTML = shown.map(([name, key], i) => {
      const at = q ? name.toLowerCase().indexOf(q) : -1;
      const label = at < 0 ? escapeHtml(name) : escapeHtml(name.slice(0, at)) + "<mark>" + escapeHtml(name.slice(at, at + q.length)) + "</mark>" + escapeHtml(name.slice(at + q.length));
      return `<li class="${i === selected ? "sel" : ""}"><span>${label}</span>${key ? `<kbd>${escapeHtml(key)}</kbd>` : ""}</li>`;
    }).join("");
    empty.hidden = shown.length > 0;
  };
  input.addEventListener("input", () => { selected = 0; render(); });
  input.addEventListener("keydown", (event) => {
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      if (!shown.length) return;
      selected = (selected + (event.key === "ArrowDown" ? 1 : -1) + shown.length) % shown.length;
      render();
      list.children[selected]?.scrollIntoView({ block: "nearest" });
    }
  });
  render();
})();

(function commitDemo() {
  const spark = document.getElementById("spark");
  const box = document.getElementById("msg-text");
  if (!spark || !box) return;
  const message = "Let overlay pages grow to their content so they can scroll";
  let timer = null;
  spark.addEventListener("click", () => {
    clearTimeout(timer);
    spark.disabled = true;
    box.textContent = "";
    if (reduceMotion) {
      box.textContent = message;
      spark.disabled = false;
      return;
    }
    box.textContent = "Writing a commit message...";
    timer = setTimeout(() => {
      box.textContent = "";
      let i = 0;
      const step = () => {
        i += 1;
        box.textContent = message.slice(0, i);
        if (i < message.length) timer = setTimeout(step, 22);
        else spark.disabled = false;
      };
      step();
    }, 700);
  });
})();

(function waitlist() {
  const form = document.getElementById("waitlist");
  if (!form) return;
  const email = form.elements.email;
  const button = form.querySelector("button");
  const status = document.getElementById("waitlist-status");
  const say = (text) => { status.textContent = text; };

  if (!WAITLIST_ENDPOINT) {
    email.disabled = true;
    button.disabled = true;
    say("The list isn't open yet. Star the repository to hear about releases.");
    return;
  }

  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    const value = email.value.trim();
    if (!email.checkValidity() || !value) {
      email.setAttribute("aria-invalid", "true");
      say("Enter an email address like you@example.com.");
      email.focus();
      return;
    }
    email.removeAttribute("aria-invalid");
    button.disabled = true;
    say("Adding you...");
    try {
      const response = await fetch(WAITLIST_ENDPOINT, {
        method: "POST",
        headers: { "Content-Type": "application/json", Accept: "application/json" },
        body: JSON.stringify({ email: value }),
      });
      if (!response.ok) throw new Error(String(response.status));
      form.reset();
      say("You're on the list. We'll write when a release lands.");
    } catch (error) {
      say("That didn't go through. Check your connection and try again.");
    } finally {
      button.disabled = false;
    }
  });
})();
