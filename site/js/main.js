// Forge landing page: the editor illustration (typing, theme switch), a filterable command palette,
// a commit-message demo, and the early-access form.

// ---------------------------------------------------------------------------
// Async CSS activation — CSP-safe replacement for the inline onload handler.
// The <link id="css-preload"> in <head> preloads style.css at high priority.
// Once the file is in cache this swaps rel="preload" → rel="stylesheet" so
// the browser applies the styles. This keeps CSP: script-src 'self' happy
// (no unsafe-inline needed) while still loading CSS asynchronously.
// ---------------------------------------------------------------------------
(function activateCss() {
  const preload = document.getElementById("css-preload");
  if (!preload) return;
  const apply = () => {
    preload.rel = "stylesheet";
  };
  if (preload.sheet) {
    // Already loaded and parsed (cache hit).
    apply();
  } else {
    preload.addEventListener("load", apply, { once: true });
  }
})();


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

  const render = () => {
    const q = input.value.trim().toLowerCase();
    shown = commands.filter(([name]) => name.toLowerCase().includes(q));
    selected = Math.min(selected, Math.max(shown.length - 1, 0));

    // Build list items with DOM APIs - no innerHTML / string evaluation.
    // textContent and createTextNode are safe-by-default; no escaping needed.
    // This keeps CSP: script-src 'self' satisfied (no unsafe-eval required).
    const frag = document.createDocumentFragment();
    shown.forEach(([name, key], i) => {
      const li = document.createElement("li");
      if (i === selected) li.className = "sel";

      const span = document.createElement("span");
      const at = q ? name.toLowerCase().indexOf(q) : -1;
      if (q && at >= 0) {
        if (at > 0) span.appendChild(document.createTextNode(name.slice(0, at)));
        const mark = document.createElement("mark");
        mark.textContent = name.slice(at, at + q.length);
        span.appendChild(mark);
        if (at + q.length < name.length) span.appendChild(document.createTextNode(name.slice(at + q.length)));
      } else {
        span.textContent = name;
      }
      li.appendChild(span);

      if (key) {
        const kbd = document.createElement("kbd");
        kbd.textContent = key;
        li.appendChild(kbd);
      }
      frag.appendChild(li);
    });

    list.replaceChildren(frag);
    empty.hidden = shown.length > 0;
  };
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

(function mobileNav() {
  const toggle = document.getElementById("nav-toggle");
  const nav = document.getElementById("nav-menu");
  if (!toggle || !nav) return;

  const closeNav = () => {
    toggle.setAttribute("aria-expanded", "false");
    nav.classList.remove("open");
    document.body.classList.remove("nav-open");
  };

  toggle.addEventListener("click", () => {
    const isExpanded = toggle.getAttribute("aria-expanded") === "true";
    toggle.setAttribute("aria-expanded", String(!isExpanded));
    nav.classList.toggle("open", !isExpanded);
    document.body.classList.toggle("nav-open", !isExpanded);
  });

  nav.querySelectorAll("a").forEach((link) => {
    link.addEventListener("click", () => {
      closeNav();
    });
  });

  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape" && nav.classList.contains("open")) {
      closeNav();
      toggle.focus();
    }
  });

  document.addEventListener("click", (e) => {
    if (nav.classList.contains("open") && !nav.contains(e.target) && !toggle.contains(e.target)) {
      closeNav();
    }
  });
})();
