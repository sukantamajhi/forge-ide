'use strict';
// Forge's extension host: a Node process that loads VS Code extensions and runs them against
// Forge's own `vscode` module (see vscode.js). It speaks one JSON object per line on
// stdin/stdout; see crates/forge-ext-host/src/host.rs for the protocol.

const fs = require('fs');
const os = require('os');
const path = require('path');
const readline = require('readline');
const Module = require('module');
const crypto = require('crypto');

// stdout is the protocol channel. Take it before anything else can write to it.
const writeProtocol = process.stdout.write.bind(process.stdout);
function send(method, params) {
  writeProtocol(JSON.stringify({ method, params }) + '\n');
}
function reply(id, result, error) {
  writeProtocol(JSON.stringify(error ? { id, error } : { id, result: result === undefined ? null : result }) + '\n');
}
function log(level, text) {
  send('log', { level, text: String(text) });
}
for (const level of ['log', 'info', 'warn', 'error', 'debug']) {
  console[level] = (...args) => log(level === 'log' ? 'info' : level, args.map((a) => (typeof a === 'string' ? a : require('util').inspect(a))).join(' '));
}
process.stdout.write = (chunk) => { log('info', Buffer.isBuffer(chunk) ? chunk.toString('utf8') : chunk); return true; };
process.stderr.write = (chunk) => { log('error', Buffer.isBuffer(chunk) ? chunk.toString('utf8') : chunk); return true; };

// ---- calls from the host to the editor ----

let nextId = 1;
const pending = new Map();
function request(method, params) {
  const id = nextId++;
  return new Promise((resolve, reject) => {
    pending.set(id, { resolve, reject });
    writeProtocol(JSON.stringify({ id, method, params }) + '\n');
  });
}

const state = {
  settings: {},
  configDefaults: {},
  folders: [],
  languages: [],
  extensions: [],
  storageDir: path.join(os.tmpdir(), 'forge-ext-storage'),
  machineId: crypto.randomBytes(16).toString('hex'),
  sessionId: crypto.randomBytes(8).toString('hex'),
};

const active = new Map(); // id -> { module, exports, context, activation: Promise }

const hostApi = {
  state,
  send,
  request,
  log,
  isActive: (id) => active.has(id) && active.get(id).done === true,
  exportsOf: (id) => (active.get(id) || {}).exports,
  activate: (id) => activate(id),
};
const vscode = require('./vscode.js')(hostApi);

// `require('vscode')` inside an extension gives it Forge's module.
const originalLoad = Module._load;
Module._load = function load(request_, parent, isMain) {
  if (request_ === 'vscode') return vscode;
  return originalLoad.call(this, request_, parent, isMain);
};

// ---- mementos (workspaceState / globalState) ----

function memento(extensionId, scope) {
  const file = path.join(state.storageDir, extensionId, `${scope}-state.json`);
  let values = {};
  try { values = JSON.parse(fs.readFileSync(file, 'utf8')); } catch (e) { /* first run */ }
  let timer;
  const persist = () => {
    clearTimeout(timer);
    timer = setTimeout(() => {
      try {
        fs.mkdirSync(path.dirname(file), { recursive: true });
        fs.writeFileSync(file, JSON.stringify(values));
      } catch (e) { log('warn', `could not save ${file}: ${e.message}`); }
    }, 100);
  };
  return {
    keys: () => Object.keys(values),
    get: (key, fallback) => (key in values ? values[key] : fallback),
    update: (key, value) => {
      if (value === undefined) delete values[key];
      else values[key] = value;
      persist();
      return Promise.resolve();
    },
  };
}

// ---- activation ----

function activate(id) {
  const key = Object.keys(Object.fromEntries(active)).find((k) => k.toLowerCase() === String(id).toLowerCase()) || id;
  if (active.has(key)) return active.get(key).activation;
  const description = state.extensions.find((e) => e.id.toLowerCase() === String(id).toLowerCase());
  if (!description) return Promise.reject(new Error(`unknown extension ${id}`));
  const entry = { done: false, exports: undefined, module: undefined, context: undefined };
  active.set(description.id, entry);
  entry.activation = (async () => {
    const main = path.resolve(description.root, description.main || '');
    entry.module = require(main);
    entry.context = vscode.__createContext(description, (scope) => memento(description.id, scope));
    if (typeof entry.module.activate === 'function') {
      entry.exports = await entry.module.activate(entry.context);
    }
    entry.done = true;
    send('extension.activated', { id: description.id });
    return entry.exports;
  })().catch((error) => {
    active.delete(description.id);
    send('extension.error', { id: description.id, message: error && error.message || String(error), stack: error && error.stack });
    throw error;
  });
  return entry.activation;
}

async function shutdown() {
  for (const [id, entry] of active) {
    try {
      if (entry.module && typeof entry.module.deactivate === 'function') {
        await Promise.race([Promise.resolve(entry.module.deactivate()), new Promise((r) => setTimeout(r, 1000))]);
      }
      for (const d of (entry.context && entry.context.subscriptions) || []) {
        try { d && d.dispose && d.dispose(); } catch (e) { /* ignore */ }
      }
    } catch (error) {
      log('warn', `${id} failed to deactivate: ${error.message}`);
    }
  }
  process.exit(0);
}

// ---- dispatch ----

const methods = {
  init(params) {
    state.settings = params.settings || {};
    state.folders = params.folders || [];
    state.languages = params.languages || [];
    state.extensions = params.extensions || [];
    if (params.storageDir) state.storageDir = params.storageDir;
    state.configDefaults = {};
    for (const e of state.extensions) Object.assign(state.configDefaults, e.configurationDefaults || {});
    return { version: process.version, pid: process.pid };
  },
  async activate(params) {
    await activate(params.id);
    return true;
  },
  executeCommand(params) {
    return vscode.__executeCommand(params.command, params.args);
  },
  hasCommand(params) {
    return vscode.__hasCommand(params.command);
  },
  shutdown() {
    shutdown();
  },
};

async function handle(message) {
  const { id, method, params } = message;
  if (!method) {
    // An answer to one of our requests.
    const waiting = pending.get(id);
    if (waiting) {
      pending.delete(id);
      if (message.error) waiting.reject(new Error(message.error.message || 'request failed'));
      else waiting.resolve(message.result);
    }
    return;
  }
  try {
    const handler = methods[method] || vscode.__handlers[method];
    if (!handler) throw new Error(`unknown method ${method}`);
    const result = await handler(params || {});
    if (id !== undefined) reply(id, result);
  } catch (error) {
    if (id !== undefined) reply(id, undefined, { message: error && error.message || String(error), stack: error && error.stack });
    else log('error', `${method} failed: ${error && error.stack || error}`);
  }
}

process.on('uncaughtException', (error) => log('error', `uncaught exception: ${error && error.stack || error}`));
process.on('unhandledRejection', (error) => log('error', `unhandled rejection: ${error && error.stack || error}`));

const input = readline.createInterface({ input: process.stdin, crlfDelay: Infinity });
input.on('line', (line) => {
  if (!line.trim()) return;
  let message;
  try { message = JSON.parse(line); } catch (error) { log('error', `bad message from Forge: ${error.message}`); return; }
  handle(message);
});
input.on('close', () => process.exit(0));
send('ready', { version: process.version, pid: process.pid });
