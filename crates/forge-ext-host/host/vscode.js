'use strict';
// Forge's implementation of the `vscode` extension API, written from the public API
// documentation. It runs inside Forge's Node extension host and talks to the editor through
// `host.send` (notifications) and `host.request` (calls that return a promise).
//
// What is real: commands, status bar items, output channels, messages, diagnostics, documents
// and their change events, configuration, workspace edits and the data classes (Uri, Range,
// Position, ...). What is not: anything that needs a UI Forge does not have yet (webviews,
// tree views, terminals, notebooks) and language feature providers (completion, hover, ...
// are accepted and ignored). Unsupported members throw "not supported by Forge yet" when
// called instead of returning undefined.

const fs = require('fs');
const os = require('os');
const path = require('path');

const WINDOWS = process.platform === 'win32';

module.exports = function createApi(host) {
  const state = host.state;

  // ---------- basics ----------

  class Disposable {
    constructor(callOnDispose) {
      this._callOnDispose = callOnDispose;
    }
    static from(...disposables) {
      return new Disposable(() => disposables.forEach((d) => d && d.dispose && d.dispose()));
    }
    dispose() {
      if (this._callOnDispose) {
        const fn = this._callOnDispose;
        this._callOnDispose = undefined;
        fn();
      }
    }
  }

  class EventEmitter {
    constructor() {
      this._listeners = new Set();
      this.event = (listener, thisArgs, disposables) => {
        const bound = thisArgs ? listener.bind(thisArgs) : listener;
        this._listeners.add(bound);
        const disposable = new Disposable(() => this._listeners.delete(bound));
        if (Array.isArray(disposables)) disposables.push(disposable);
        return disposable;
      };
    }
    fire(data) {
      for (const listener of [...this._listeners]) {
        try {
          listener(data);
        } catch (error) {
          host.log('error', `an event listener threw: ${error && error.stack || error}`);
        }
      }
    }
    dispose() {
      this._listeners.clear();
    }
  }

  class CancellationTokenSource {
    constructor() {
      this._emitter = new EventEmitter();
      this.token = { isCancellationRequested: false, onCancellationRequested: this._emitter.event };
    }
    cancel() {
      if (!this.token.isCancellationRequested) {
        this.token.isCancellationRequested = true;
        this._emitter.fire();
      }
    }
    dispose() {
      this._emitter.dispose();
    }
  }
  const CancellationToken = {
    None: Object.freeze({ isCancellationRequested: false, onCancellationRequested: () => new Disposable(() => {}) }),
    Cancelled: Object.freeze({ isCancellationRequested: true, onCancellationRequested: () => new Disposable(() => {}) }),
  };

  class Position {
    constructor(line, character) {
      if (line < 0 || character < 0) throw new Error('Position line and character must be non-negative');
      this.line = line;
      this.character = character;
    }
    isBefore(o) { return this.compareTo(o) < 0; }
    isBeforeOrEqual(o) { return this.compareTo(o) <= 0; }
    isAfter(o) { return this.compareTo(o) > 0; }
    isAfterOrEqual(o) { return this.compareTo(o) >= 0; }
    isEqual(o) { return this.compareTo(o) === 0; }
    compareTo(o) { return this.line !== o.line ? this.line - o.line : this.character - o.character; }
    translate(lineDelta, characterDelta) {
      if (typeof lineDelta === 'object') return this.translate(lineDelta.lineDelta, lineDelta.characterDelta);
      return new Position(this.line + (lineDelta || 0), this.character + (characterDelta || 0));
    }
    with(line, character) {
      if (typeof line === 'object') return this.with(line.line, line.character);
      return new Position(line === undefined ? this.line : line, character === undefined ? this.character : character);
    }
  }

  class Range {
    constructor(a, b, c, d) {
      let start, end;
      if (typeof a === 'number') {
        start = new Position(a, b);
        end = new Position(c, d);
      } else {
        start = a;
        end = b;
      }
      if (start.isAfter(end)) [start, end] = [end, start];
      this.start = start;
      this.end = end;
    }
    get isEmpty() { return this.start.isEqual(this.end); }
    get isSingleLine() { return this.start.line === this.end.line; }
    contains(x) {
      if (x instanceof Range) return this.contains(x.start) && this.contains(x.end);
      return !(x.isBefore(this.start) || x.isAfter(this.end));
    }
    isEqual(o) { return this.start.isEqual(o.start) && this.end.isEqual(o.end); }
    intersection(o) {
      const start = this.start.isAfter(o.start) ? this.start : o.start;
      const end = this.end.isBefore(o.end) ? this.end : o.end;
      return start.isAfter(end) ? undefined : new Range(start, end);
    }
    union(o) {
      return new Range(this.start.isBefore(o.start) ? this.start : o.start, this.end.isAfter(o.end) ? this.end : o.end);
    }
    with(start, end) {
      if (start && !(start instanceof Position)) return this.with(start.start, start.end);
      return new Range(start || this.start, end || this.end);
    }
  }

  class Selection extends Range {
    constructor(a, b, c, d) {
      let anchor, active;
      if (typeof a === 'number') {
        anchor = new Position(a, b);
        active = new Position(c, d);
      } else {
        anchor = a;
        active = b;
      }
      super(anchor, active);
      this.anchor = anchor;
      this.active = active;
    }
    get isReversed() { return this.anchor.isAfter(this.active); }
  }

  // ---------- Uri ----------

  function encodePath(p) {
    return p.split('/').map(encodeURIComponent).join('/');
  }

  class Uri {
    constructor(scheme, authority, uriPath, query, fragment) {
      this.scheme = scheme || 'file';
      this.authority = authority || '';
      this.path = uriPath || '';
      this.query = query || '';
      this.fragment = fragment || '';
    }
    static file(p) {
      p = String(p).replace(/\\/g, '/');
      let authority = '';
      if (p.startsWith('//')) {
        const slash = p.indexOf('/', 2);
        authority = slash === -1 ? p.slice(2) : p.slice(2, slash);
        p = slash === -1 ? '/' : p.slice(slash);
      } else if (!p.startsWith('/')) {
        p = '/' + p;
      }
      return new Uri('file', authority, p, '', '');
    }
    static parse(value) {
      const match = /^(([^:/?#]+?):)?(\/\/([^/?#]*))?([^?#]*)(\?([^#]*))?(#(.*))?/.exec(String(value));
      const decode = (s) => { try { return decodeURIComponent(s || ''); } catch (e) { return s || ''; } };
      const scheme = match[2] || 'file';
      let uriPath = decode(match[5]);
      if ((scheme === 'file' || scheme === 'https' || scheme === 'http') && uriPath && !uriPath.startsWith('/')) uriPath = '/' + uriPath;
      return new Uri(scheme, decode(match[4]), uriPath, decode(match[7]), decode(match[9]));
    }
    static from(c) {
      return new Uri(c.scheme, c.authority, c.path, c.query, c.fragment);
    }
    static joinPath(base, ...segments) {
      return base.with({ path: path.posix.join(base.path || '/', ...segments) });
    }
    get fsPath() {
      let value;
      if (this.authority && this.path.length > 1 && this.scheme === 'file') value = `//${this.authority}${this.path}`;
      else if (/^\/[a-zA-Z]:/.test(this.path)) value = this.path[1].toLowerCase() + this.path.slice(2);
      else value = this.path;
      return WINDOWS ? value.replace(/\//g, '\\') : value;
    }
    with(change) {
      return new Uri(
        change.scheme === undefined ? this.scheme : change.scheme,
        change.authority === undefined ? this.authority : change.authority,
        change.path === undefined ? this.path : change.path,
        change.query === undefined ? this.query : change.query,
        change.fragment === undefined ? this.fragment : change.fragment,
      );
    }
    toString() {
      let out = this.scheme + ':';
      if (this.authority || this.scheme === 'file') out += '//' + this.authority;
      out += encodePath(this.path);
      if (this.query) out += '?' + encodeURIComponent(this.query);
      if (this.fragment) out += '#' + encodeURIComponent(this.fragment);
      return out;
    }
    toJSON() {
      return { $mid: 1, fsPath: this.fsPath, external: this.toString(), path: this.path, scheme: this.scheme, authority: this.authority, query: this.query, fragment: this.fragment };
    }
  }

  /** The key two paths share when they name the same file. */
  function pathKey(p) {
    const normal = path.normalize(p);
    return WINDOWS ? normal.toLowerCase() : normal;
  }

  // ---------- data classes ----------

  const DiagnosticSeverity = Object.freeze({ Error: 0, Warning: 1, Information: 2, Hint: 3, 0: 'Error', 1: 'Warning', 2: 'Information', 3: 'Hint' });
  const DiagnosticTag = Object.freeze({ Unnecessary: 1, Deprecated: 2 });
  class Diagnostic {
    constructor(range, message, severity) {
      this.range = range;
      this.message = message;
      this.severity = severity === undefined ? DiagnosticSeverity.Error : severity;
    }
  }
  class DiagnosticRelatedInformation {
    constructor(location, message) {
      this.location = location;
      this.message = message;
    }
  }
  class Location {
    constructor(uri, rangeOrPosition) {
      this.uri = uri;
      this.range = rangeOrPosition instanceof Position ? new Range(rangeOrPosition, rangeOrPosition) : rangeOrPosition;
    }
  }
  class TextEdit {
    constructor(range, newText) {
      this.range = range;
      this.newText = newText;
    }
    static replace(range, newText) { return new TextEdit(range, newText); }
    static insert(position, newText) { return new TextEdit(new Range(position, position), newText); }
    static delete(range) { return new TextEdit(range, ''); }
    static setEndOfLine() { return new TextEdit(new Range(0, 0, 0, 0), ''); }
  }
  class WorkspaceEdit {
    constructor() {
      this._edits = new Map();
    }
    _for(uri) {
      const key = uri.toString();
      if (!this._edits.has(key)) this._edits.set(key, { uri, edits: [] });
      return this._edits.get(key).edits;
    }
    replace(uri, range, newText) { this._for(uri).push(new TextEdit(range, newText)); }
    insert(uri, position, newText) { this._for(uri).push(TextEdit.insert(position, newText)); }
    delete(uri, range) { this._for(uri).push(TextEdit.delete(range)); }
    has(uri) { return this._edits.has(uri.toString()); }
    set(uri, edits) { this._edits.set(uri.toString(), { uri, edits: edits.map((e) => (Array.isArray(e) ? e[0] : e)) }); }
    get(uri) { const entry = this._edits.get(uri.toString()); return entry ? entry.edits : []; }
    entries() { return [...this._edits.values()].map((e) => [e.uri, e.edits]); }
    get size() { return this._edits.size; }
  }
  class MarkdownString {
    constructor(value, supportThemeIcons) {
      this.value = value || '';
      this.supportThemeIcons = supportThemeIcons;
    }
    appendText(v) { this.value += v; return this; }
    appendMarkdown(v) { this.value += v; return this; }
    appendCodeblock(code, language = '') { this.value += '\n```' + language + '\n' + code + '\n```\n'; return this; }
  }
  class SnippetString {
    constructor(value) { this.value = value || ''; }
    appendText(v) { this.value += v; return this; }
    appendTabstop() { return this; }
    appendPlaceholder() { return this; }
  }
  class ThemeColor {
    constructor(id) { this.id = id; }
  }
  class ThemeIcon {
    constructor(id, color) { this.id = id; this.color = color; }
  }
  ThemeIcon.File = new ThemeIcon('file');
  ThemeIcon.Folder = new ThemeIcon('folder');
  class Hover {
    constructor(contents, range) { this.contents = Array.isArray(contents) ? contents : [contents]; this.range = range; }
  }
  class CompletionItem {
    constructor(label, kind) { this.label = label; this.kind = kind; }
  }
  class CompletionList {
    constructor(items, isIncomplete) { this.items = items || []; this.isIncomplete = !!isIncomplete; }
  }
  class CodeAction {
    constructor(title, kind) { this.title = title; this.kind = kind; }
  }
  class CodeLens {
    constructor(range, command) { this.range = range; this.command = command; this.isResolved = !!command; }
  }
  class DocumentSymbol {
    constructor(name, detail, kind, range, selectionRange) { Object.assign(this, { name, detail, kind, range, selectionRange, children: [] }); }
  }
  class SymbolInformation {
    constructor(name, kind, containerName, location) { Object.assign(this, { name, kind, containerName, location }); }
  }
  class FoldingRange {
    constructor(start, end, kind) { Object.assign(this, { start, end, kind }); }
  }
  class TreeItem {
    constructor(label, collapsibleState) { this.label = label; this.collapsibleState = collapsibleState; }
  }
  class RelativePattern {
    constructor(base, pattern) { this.baseUri = typeof base === 'string' ? Uri.file(base) : base.uri || base; this.base = this.baseUri.fsPath; this.pattern = pattern; }
  }
  class FileSystemError extends Error {
    constructor(messageOrUri) { super(typeof messageOrUri === 'string' ? messageOrUri : String(messageOrUri)); this.code = 'Unknown'; }
    static FileNotFound(m) { const e = new FileSystemError(m || 'File not found'); e.code = 'FileNotFound'; return e; }
    static FileExists(m) { const e = new FileSystemError(m || 'File exists'); e.code = 'FileExists'; return e; }
    static NoPermissions(m) { const e = new FileSystemError(m || 'No permissions'); e.code = 'NoPermissions'; return e; }
  }

  const enums = {
    StatusBarAlignment: Object.freeze({ Left: 1, Right: 2 }),
    EndOfLine: Object.freeze({ LF: 1, CRLF: 2 }),
    ViewColumn: Object.freeze({ Active: -1, Beside: -2, One: 1, Two: 2, Three: 3 }),
    ConfigurationTarget: Object.freeze({ Global: 1, Workspace: 2, WorkspaceFolder: 3 }),
    TextDocumentSaveReason: Object.freeze({ Manual: 1, AfterDelay: 2, FocusOut: 3 }),
    TextDocumentChangeReason: Object.freeze({ Undo: 1, Redo: 2 }),
    ExtensionMode: Object.freeze({ Production: 1, Development: 2, Test: 3 }),
    FileType: Object.freeze({ Unknown: 0, File: 1, Directory: 2, SymbolicLink: 64 }),
    UIKind: Object.freeze({ Desktop: 1, Web: 2 }),
    ProgressLocation: Object.freeze({ SourceControl: 1, Window: 10, Notification: 15 }),
    TreeItemCollapsibleState: Object.freeze({ None: 0, Collapsed: 1, Expanded: 2 }),
    CompletionItemKind: Object.freeze({ Text: 0, Method: 1, Function: 2, Constructor: 3, Field: 4, Variable: 5, Class: 6, Interface: 7, Module: 8, Property: 9, Unit: 10, Value: 11, Enum: 12, Keyword: 13, Snippet: 14, Color: 15, File: 16, Reference: 17, Folder: 18, EnumMember: 19, Constant: 20, Struct: 21, Event: 22, Operator: 23, TypeParameter: 24 }),
    SymbolKind: Object.freeze({ File: 0, Module: 1, Namespace: 2, Package: 3, Class: 4, Method: 5, Property: 6, Field: 7, Constructor: 8, Enum: 9, Interface: 10, Function: 11, Variable: 12, Constant: 13, String: 14, Number: 15, Boolean: 16, Array: 17, Object: 18, Key: 19, Null: 20, EnumMember: 21, Struct: 22, Event: 23, Operator: 24, TypeParameter: 25 }),
    CodeActionKind: Object.freeze({ Empty: '', QuickFix: 'quickfix', Refactor: 'refactor', Source: 'source', SourceOrganizeImports: 'source.organizeImports', SourceFixAll: 'source.fixAll' }),
    OverviewRulerLane: Object.freeze({ Left: 1, Center: 2, Right: 4, Full: 7 }),
    DecorationRangeBehavior: Object.freeze({ OpenOpen: 0, ClosedClosed: 1, OpenClosed: 2, ClosedOpen: 3 }),
    TextEditorRevealType: Object.freeze({ Default: 0, InCenter: 1, InCenterIfOutsideViewport: 2, AtTop: 3 }),
    DiagnosticSeverity,
    DiagnosticTag,
  };

  // ---------- documents ----------

  const EOL_PATTERN = /\r\n|\r|\n/;

  class TextDocumentImpl {
    constructor({ path: fsPath, languageId, version, text }) {
      this.uri = Uri.file(fsPath);
      this.fileName = this.uri.fsPath;
      this.languageId = languageId || 'plaintext';
      this.version = version || 1;
      this.isUntitled = false;
      this.isDirty = false;
      this.isClosed = false;
      this._setText(text || '');
    }
    _setText(text) {
      this._text = text;
      this._lines = text.split(EOL_PATTERN);
      this.eol = text.includes('\r\n') ? enums.EndOfLine.CRLF : enums.EndOfLine.LF;
      // Offsets of each line start, for offsetAt/positionAt.
      this._starts = [0];
      const re = /\r\n|\r|\n/g;
      let m;
      while ((m = re.exec(text))) this._starts.push(m.index + m[0].length);
    }
    get lineCount() { return this._lines.length; }
    get encoding() { return 'utf8'; }
    save() { return host.request('document.save', { path: this.fileName }).then(() => true, () => false); }
    getText(range) {
      if (!range) return this._text;
      const start = this.offsetAt(range.start);
      const end = this.offsetAt(range.end);
      return this._text.slice(start, end);
    }
    lineAt(lineOrPosition) {
      const line = typeof lineOrPosition === 'number' ? lineOrPosition : lineOrPosition.line;
      if (line < 0 || line >= this._lines.length) throw new Error('Illegal value for `line`');
      const text = this._lines[line];
      const range = new Range(line, 0, line, text.length);
      const next = line + 1 < this._lines.length ? new Position(line + 1, 0) : range.end;
      const first = text.search(/\S/);
      return { lineNumber: line, text, range, rangeIncludingLineBreak: new Range(range.start, next), firstNonWhitespaceCharacterIndex: first === -1 ? text.length : first, isEmptyOrWhitespace: first === -1 };
    }
    offsetAt(position) {
      const p = this.validatePosition(position);
      return this._starts[p.line] + p.character;
    }
    positionAt(offset) {
      offset = Math.max(0, Math.min(offset, this._text.length));
      let low = 0;
      let high = this._starts.length - 1;
      while (low < high) {
        const mid = (low + high + 1) >> 1;
        if (this._starts[mid] <= offset) low = mid;
        else high = mid - 1;
      }
      return new Position(low, offset - this._starts[low]);
    }
    validatePosition(position) {
      const line = Math.max(0, Math.min(position.line, this._lines.length - 1));
      return new Position(line, Math.max(0, Math.min(position.character, this._lines[line].length)));
    }
    validateRange(range) {
      return new Range(this.validatePosition(range.start), this.validatePosition(range.end));
    }
    getWordRangeAtPosition(position, regex) {
      const line = this._lines[position.line] || '';
      const re = new RegExp((regex || /-?\d*\.\d\w*|[^\`\~\!\@\#\%\^\&\*\(\)\-\=\+\[\{\]\}\\\|\;\:\'\"\,\.\<\>\/\?\s]+/g).source, 'g');
      let m;
      while ((m = re.exec(line))) {
        if (m.index <= position.character && position.character <= m.index + m[0].length) {
          return new Range(position.line, m.index, position.line, m.index + m[0].length);
        }
        if (m[0].length === 0) re.lastIndex++;
      }
      return undefined;
    }
  }

  const documents = new Map(); // pathKey -> TextDocumentImpl
  const onDidOpen = new EventEmitter();
  const onDidClose = new EventEmitter();
  const onDidChange = new EventEmitter();
  const onDidSave = new EventEmitter();
  const onDidChangeConfig = new EventEmitter();
  const onDidChangeActiveEditor = new EventEmitter();
  const onDidChangeSelection = new EventEmitter();

  let activeEditor;

  function makeEditor(document, selections) {
    const sels = (selections && selections.length ? selections : [[0, 0, 0, 0]]).map((s) => new Selection(s[0], s[1], s[2], s[3]));
    const editor = {
      document,
      selections: sels,
      get selection() { return this.selections[0]; },
      set selection(value) { this.selections = [value]; },
      visibleRanges: [new Range(0, 0, Math.min(document.lineCount, 50), 0)],
      options: { tabSize: 4, insertSpaces: true },
      viewColumn: enums.ViewColumn.One,
      edit(callback) {
        const builder = {
          replace: (where, text) => collected.push(TextEdit.replace(where instanceof Position ? new Range(where, where) : where, text)),
          insert: (position, text) => collected.push(TextEdit.insert(position, text)),
          delete: (range) => collected.push(TextEdit.delete(range)),
          setEndOfLine: () => {},
        };
        const collected = [];
        callback(builder);
        const edit = new WorkspaceEdit();
        edit.set(document.uri, collected);
        return api.workspace.applyEdit(edit);
      },
      insertSnippet(snippet) {
        const edit = new WorkspaceEdit();
        edit.insert(document.uri, this.selection.active, snippet.value.replace(/\$\{?\d+(:([^}]*))?\}?/g, (_, __, d) => d || ''));
        return api.workspace.applyEdit(edit);
      },
      revealRange() {},
      setDecorations() {},
      show() {},
      hide() {},
    };
    return editor;
  }

  function openDocument(payload) {
    const key = pathKey(payload.path);
    let doc = documents.get(key);
    if (doc) {
      doc._setText(payload.text);
      doc.version = payload.version;
      doc.languageId = payload.languageId || doc.languageId;
      return doc;
    }
    doc = new TextDocumentImpl(payload);
    documents.set(key, doc);
    onDidOpen.fire(doc);
    return doc;
  }

  /** The one edit that turns `before` into `after`: their common prefix and suffix are kept. */
  function diffChange(doc, before, after) {
    let start = 0;
    const max = Math.min(before.length, after.length);
    while (start < max && before.charCodeAt(start) === after.charCodeAt(start)) start++;
    let endBefore = before.length;
    let endAfter = after.length;
    while (endBefore > start && endAfter > start && before.charCodeAt(endBefore - 1) === after.charCodeAt(endAfter - 1)) {
      endBefore--;
      endAfter--;
    }
    return { start, end: endBefore, text: after.slice(start, endAfter) };
  }

  // ---------- messages from the editor ----------

  const handlers = {
    'doc.open': (p) => {
      openDocument(p);
    },
    'doc.change': (p) => {
      const doc = documents.get(pathKey(p.path));
      if (!doc) {
        openDocument(p);
        return;
      }
      const before = doc._text;
      const change = diffChange(doc, before, p.text);
      const range = new Range(doc.positionAt(change.start), doc.positionAt(change.end));
      doc._setText(p.text);
      doc.version = p.version;
      doc.isDirty = true;
      onDidChange.fire({ document: doc, reason: undefined, contentChanges: [{ range, rangeOffset: change.start, rangeLength: change.end - change.start, text: change.text }] });
    },
    'doc.save': (p) => {
      const doc = documents.get(pathKey(p.path));
      if (doc) {
        doc.isDirty = false;
        onDidSave.fire(doc);
      }
    },
    'doc.close': (p) => {
      const key = pathKey(p.path);
      const doc = documents.get(key);
      if (doc) {
        doc.isClosed = true;
        documents.delete(key);
        onDidClose.fire(doc);
      }
    },
    'editor.active': (p) => {
      const doc = p && p.path ? documents.get(pathKey(p.path)) : undefined;
      const previous = activeEditor;
      activeEditor = doc ? makeEditor(doc, p.selections) : undefined;
      if (previous !== activeEditor) onDidChangeActiveEditor.fire(activeEditor);
      if (activeEditor) onDidChangeSelection.fire({ textEditor: activeEditor, selections: activeEditor.selections, kind: undefined });
    },
    'config.changed': (p) => {
      state.settings = p.settings || {};
      onDidChangeConfig.fire({ affectsConfiguration: () => true });
    },
  };

  // ---------- configuration ----------

  function lookup(section, key) {
    const full = section ? `${section}.${key}` : key;
    for (const source of [state.settings, state.configDefaults]) {
      if (full in source) return { found: true, value: source[full] };
      // A value stored as a nested object under a shorter key.
      const parts = full.split('.');
      for (let i = parts.length - 1; i > 0; i--) {
        const head = parts.slice(0, i).join('.');
        if (head in source && source[head] && typeof source[head] === 'object') {
          let value = source[head];
          let ok = true;
          for (const part of parts.slice(i)) {
            if (value && typeof value === 'object' && part in value) value = value[part];
            else { ok = false; break; }
          }
          if (ok) return { found: true, value };
        }
      }
    }
    return { found: false };
  }

  function getConfiguration(section) {
    const prefix = section ? `${section}.` : '';
    const config = {
      get(key, defaultValue) {
        const hit = lookup(section, key);
        return hit.found ? hit.value : defaultValue;
      },
      has(key) { return lookup(section, key).found; },
      inspect(key) {
        const user = key in state.settings || `${prefix}${key}` in state.settings;
        const hit = lookup(section, key);
        return { key: prefix + key, defaultValue: state.configDefaults[prefix + key], globalValue: user ? hit.value : undefined };
      },
      update(key, value) {
        return host.request('config.update', { key: prefix + key, value }).then(() => undefined, () => undefined);
      },
    };
    return new Proxy(config, {
      get(target, prop) {
        if (prop in target || typeof prop === 'symbol') return target[prop];
        const hit = lookup(section, String(prop));
        return hit.found ? hit.value : undefined;
      },
    });
  }

  // ---------- status bar, output, messages ----------

  let nextStatusId = 1;
  const statusItems = new Map();
  class StatusBarItem {
    constructor(alignment, priority, id) {
      this.id = id || `status.${nextStatusId++}`;
      this.alignment = alignment || enums.StatusBarAlignment.Left;
      this.priority = priority || 0;
      this._text = '';
      this._tooltip = undefined;
      this._command = undefined;
      this._visible = false;
      this.name = undefined;
      this.color = undefined;
      this.backgroundColor = undefined;
      this.accessibilityInformation = undefined;
      this._queued = false;
      statusItems.set(this.id, this);
    }
    get text() { return this._text; }
    set text(v) { this._text = String(v); this._queue(); }
    get tooltip() { return this._tooltip; }
    set tooltip(v) { this._tooltip = v; this._queue(); }
    get command() { return this._command; }
    set command(v) { this._command = v; this._queue(); }
    show() { this._visible = true; this._queue(); }
    hide() { this._visible = false; this._queue(); }
    dispose() {
      this._visible = false;
      statusItems.delete(this.id);
      host.send('status.dispose', { id: this.id });
    }
    _queue() {
      if (this._queued) return;
      this._queued = true;
      setImmediate(() => {
        this._queued = false;
        if (!statusItems.has(this.id)) return;
        const tip = this._tooltip && typeof this._tooltip === 'object' ? this._tooltip.value : this._tooltip;
        const command = this._command && typeof this._command === 'object' ? this._command.command : this._command;
        host.send('status.update', { id: this.id, text: this._text, tooltip: tip || null, command: command || null, alignment: this.alignment, priority: this.priority, visible: this._visible });
      });
    }
  }

  class OutputChannel {
    constructor(name, log) {
      this.name = name;
      if (log) {
        for (const level of ['trace', 'debug', 'info', 'warn', 'error']) {
          this[level] = (message, ...args) => this.appendLine(`[${level}] ${[message, ...args].map(String).join(' ')}`);
        }
        this.logLevel = 2;
        this.onDidChangeLogLevel = new EventEmitter().event;
      }
    }
    append(text) { host.send('output.append', { channel: this.name, text: String(text) }); }
    appendLine(text) { this.append(String(text) + '\n'); }
    replace(text) { this.clear(); this.append(text); }
    clear() { host.send('output.clear', { channel: this.name }); }
    show() { host.send('output.show', { channel: this.name }); }
    hide() {}
    dispose() {}
  }

  function showMessage(kind) {
    return (message, ...rest) => {
      const items = rest.filter((r) => typeof r === 'string' || (r && typeof r === 'object' && 'title' in r));
      host.send('window.message', { kind, text: String(message), items: items.map((i) => (typeof i === 'string' ? i : i.title)) });
      // Forge shows messages in its status bar and has no buttons, so nothing is ever chosen.
      return Promise.resolve(undefined);
    };
  }

  // ---------- diagnostics ----------

  function toWire(diagnostic) {
    const range = diagnostic.range;
    const message = diagnostic.message && typeof diagnostic.message === 'object' ? diagnostic.message.value : diagnostic.message;
    const code = diagnostic.code && typeof diagnostic.code === 'object' ? diagnostic.code.value : diagnostic.code;
    return { range: [range.start.line, range.start.character, range.end.line, range.end.character], message: String(message), severity: diagnostic.severity === undefined ? 0 : diagnostic.severity, source: diagnostic.source || null, code: code === undefined ? null : String(code) };
  }

  class DiagnosticCollection {
    constructor(name) {
      this.name = name;
      this._entries = new Map(); // fsPath -> { uri, diagnostics }
      this._dirty = new Set();
      this._queued = false;
    }
    set(uriOrEntries, diagnostics) {
      if (Array.isArray(uriOrEntries)) {
        for (const [uri, list] of uriOrEntries) this.set(uri, list);
        return;
      }
      const uri = uriOrEntries;
      if (!diagnostics) {
        this.delete(uri);
        return;
      }
      this._entries.set(uri.fsPath, { uri, diagnostics: [...diagnostics] });
      this._touch(uri.fsPath);
    }
    delete(uri) {
      if (this._entries.delete(uri.fsPath)) this._touch(uri.fsPath);
    }
    clear() {
      for (const key of [...this._entries.keys()]) this._touch(key);
      this._entries.clear();
    }
    forEach(callback, thisArg) {
      for (const { uri, diagnostics } of this._entries.values()) callback.call(thisArg, uri, diagnostics, this);
    }
    get(uri) { const e = this._entries.get(uri.fsPath); return e ? e.diagnostics : undefined; }
    has(uri) { return this._entries.has(uri.fsPath); }
    dispose() { this.clear(); this._flush(); }
    _touch(key) {
      this._dirty.add(key);
      if (this._queued) return;
      this._queued = true;
      setImmediate(() => this._flush());
    }
    _flush() {
      this._queued = false;
      const entries = [...this._dirty].map((key) => {
        const entry = this._entries.get(key);
        return { path: key, diagnostics: entry ? entry.diagnostics.map(toWire) : [] };
      });
      this._dirty.clear();
      if (entries.length) host.send('diagnostics.set', { collection: this.name, entries });
    }
  }

  // ---------- files ----------

  function globToRegExp(glob) {
    let out = '';
    for (let i = 0; i < glob.length; i++) {
      const c = glob[i];
      if (c === '*') {
        if (glob[i + 1] === '*') {
          i++;
          if (glob[i + 1] === '/') { i++; out += '(?:.*/)?'; } else out += '.*';
        } else out += '[^/]*';
      } else if (c === '?') out += '[^/]';
      else if (c === '{') {
        const end = glob.indexOf('}', i);
        if (end === -1) out += '\\{';
        else { out += '(?:' + glob.slice(i + 1, end).split(',').map((s) => s.replace(/[.+^$()|[\]\\]/g, '\\$&')).join('|') + ')'; i = end; }
      } else out += c.replace(/[.+^$()|[\]\\]/g, '\\$&');
    }
    return new RegExp('^' + out + '$', 'i');
  }

  const fsApi = {
    async readFile(uri) { return new Uint8Array(await fs.promises.readFile(uri.fsPath)); },
    async writeFile(uri, content) { await fs.promises.writeFile(uri.fsPath, content); },
    async stat(uri) {
      const s = await fs.promises.stat(uri.fsPath);
      return { type: s.isDirectory() ? 2 : 1, ctime: s.ctimeMs, mtime: s.mtimeMs, size: s.size };
    },
    async readDirectory(uri) {
      const entries = await fs.promises.readdir(uri.fsPath, { withFileTypes: true });
      return entries.map((e) => [e.name, e.isDirectory() ? 2 : e.isSymbolicLink() ? 64 : 1]);
    },
    async createDirectory(uri) { await fs.promises.mkdir(uri.fsPath, { recursive: true }); },
    async delete(uri, options) { await fs.promises.rm(uri.fsPath, { recursive: !!(options && options.recursive), force: true }); },
    async rename(from, to) { await fs.promises.rename(from.fsPath, to.fsPath); },
    async copy(from, to) { await fs.promises.copyFile(from.fsPath, to.fsPath); },
    isWritableFileSystem: () => true,
  };

  async function findFiles(include, exclude, maxResults) {
    const folders = state.folders;
    if (!folders.length) return [];
    const base = include instanceof RelativePattern ? include.base : folders[0];
    const includePattern = include instanceof RelativePattern ? include.pattern : String(include);
    const includeRe = globToRegExp(includePattern);
    const excludeRe = exclude ? globToRegExp(typeof exclude === 'string' ? exclude : exclude.pattern || '') : null;
    const skip = new Set(['node_modules', '.git', 'target']);
    const results = [];
    async function walk(dir, relative) {
      if (results.length >= (maxResults || 5000)) return;
      let entries;
      try { entries = await fs.promises.readdir(dir, { withFileTypes: true }); } catch (e) { return; }
      for (const e of entries) {
        const rel = relative ? `${relative}/${e.name}` : e.name;
        if (e.isDirectory()) {
          if (!skip.has(e.name) && !(excludeRe && excludeRe.test(rel))) await walk(path.join(dir, e.name), rel);
        } else if (includeRe.test(rel) && !(excludeRe && excludeRe.test(rel))) {
          results.push(Uri.file(path.join(dir, e.name)));
          if (results.length >= (maxResults || 5000)) return;
        }
      }
    }
    await walk(base, '');
    return results;
  }

  // ---------- commands ----------

  const commandHandlers = new Map();

  const commands = {
    registerCommand(id, callback, thisArg) {
      if (commandHandlers.has(id)) throw new Error(`command '${id}' already exists`);
      commandHandlers.set(id, thisArg ? callback.bind(thisArg) : callback);
      host.send('commands.registered', { id });
      return new Disposable(() => {
        commandHandlers.delete(id);
        host.send('commands.unregistered', { id });
      });
    },
    registerTextEditorCommand(id, callback, thisArg) {
      return commands.registerCommand(id, (...args) => {
        if (!activeEditor) return undefined;
        return callback.call(thisArg, activeEditor, { replace() {}, insert() {}, delete() {} }, ...args);
      });
    },
    async executeCommand(id, ...args) {
      const local = commandHandlers.get(id);
      if (local) return local(...args);
      // Not ours: let Forge run it (a built-in command, or another extension's).
      return host.request('commands.execute', { id, args: JSON.parse(JSON.stringify(args, (k, v) => (v instanceof Uri ? v.toString() : v))) });
    },
    async getCommands() { return [...commandHandlers.keys()]; },
  };

  // ---------- extension bookkeeping ----------

  const extensionApis = new Map();
  function extensionInfo(id) {
    const description = state.extensions.find((e) => e.id.toLowerCase() === String(id).toLowerCase());
    if (!description) return undefined;
    if (!extensionApis.has(description.id)) {
      extensionApis.set(description.id, {
        id: description.id,
        extensionUri: Uri.file(description.root),
        extensionPath: description.root,
        packageJSON: description.packageJSON,
        extensionKind: 1,
        get isActive() { return host.isActive(description.id); },
        get exports() { return host.exportsOf(description.id); },
        activate: () => host.activate(description.id).then(() => host.exportsOf(description.id)),
      });
    }
    return extensionApis.get(description.id);
  }

  // ---------- the API object ----------

  const noopDisposable = () => new Disposable(() => {});
  function unsupported(name) {
    return () => { throw new Error(`${name} is not supported by Forge yet`); };
  }
  const warnedProviders = new Set();
  function provider(name) {
    return () => {
      if (!warnedProviders.has(name)) {
        warnedProviders.add(name);
        host.log('warn', `${name} is accepted but not used: Forge does not call extension language providers yet`);
      }
      return noopDisposable();
    };
  }

  const window = {
    showInformationMessage: showMessage('info'),
    showWarningMessage: showMessage('warning'),
    showErrorMessage: showMessage('error'),
    showInputBox: () => Promise.resolve(undefined),
    showQuickPick: () => Promise.resolve(undefined),
    showOpenDialog: () => Promise.resolve(undefined),
    showSaveDialog: () => Promise.resolve(undefined),
    showWorkspaceFolderPick: () => Promise.resolve(undefined),
    createOutputChannel: (name, options) => new OutputChannel(name, options && typeof options === 'object' && options.log),
    createStatusBarItem(a, b, c) {
      return typeof a === 'string' ? new StatusBarItem(b, c, a) : new StatusBarItem(a, b);
    },
    setStatusBarMessage(text, hideAfter) {
      const item = new StatusBarItem(enums.StatusBarAlignment.Left, 1000);
      item.text = text;
      item.show();
      const dispose = () => item.dispose();
      if (typeof hideAfter === 'number') setTimeout(dispose, hideAfter);
      else if (hideAfter && typeof hideAfter.then === 'function') hideAfter.then(dispose, dispose);
      return new Disposable(dispose);
    },
    withProgress(options, task) {
      const title = options && options.title ? String(options.title) : '';
      if (title) host.send('window.message', { kind: 'info', text: title, items: [] });
      return Promise.resolve(task({ report: (value) => { if (value && value.message) host.send('window.message', { kind: 'info', text: `${title} ${value.message}`.trim(), items: [] }); } }, CancellationToken.None));
    },
    async showTextDocument(documentOrUri) {
      const uri = documentOrUri.uri || documentOrUri;
      await host.request('window.showTextDocument', { path: uri.fsPath });
      return activeEditor;
    },
    createTextEditorDecorationType: () => ({ key: 'decoration', dispose() {} }),
    registerTreeDataProvider: noopDisposable,
    createTreeView: unsupported('window.createTreeView'),
    createWebviewPanel: unsupported('window.createWebviewPanel'),
    createTerminal: unsupported('window.createTerminal'),
    registerWebviewViewProvider: noopDisposable,
    onDidChangeActiveTextEditor: onDidChangeActiveEditor.event,
    onDidChangeTextEditorSelection: onDidChangeSelection.event,
    onDidChangeVisibleTextEditors: new EventEmitter().event,
    onDidChangeWindowState: new EventEmitter().event,
    state: { focused: true, active: true },
    get activeTextEditor() { return activeEditor; },
    get visibleTextEditors() { return activeEditor ? [activeEditor] : []; },
  };

  const workspace = {
    get workspaceFolders() {
      return state.folders.length ? state.folders.map((f, index) => ({ uri: Uri.file(f), name: path.basename(f), index })) : undefined;
    },
    get name() { return state.folders.length ? path.basename(state.folders[0]) : undefined; },
    get rootPath() { return state.folders[0]; },
    get textDocuments() { return [...documents.values()]; },
    get isTrusted() { return true; },
    getWorkspaceFolder(uri) {
      const folder = state.folders.find((f) => pathKey(uri.fsPath).startsWith(pathKey(f)));
      return folder ? { uri: Uri.file(folder), name: path.basename(folder), index: 0 } : undefined;
    },
    asRelativePath(pathOrUri, includeFolder) {
      const p = typeof pathOrUri === 'string' ? pathOrUri : pathOrUri.fsPath;
      const folder = state.folders.find((f) => pathKey(p).startsWith(pathKey(f)));
      if (!folder) return p;
      const rel = path.relative(folder, p).replace(/\\/g, '/');
      return includeFolder ? `${path.basename(folder)}/${rel}` : rel;
    },
    getConfiguration,
    onDidChangeConfiguration: onDidChangeConfig.event,
    onDidOpenTextDocument: onDidOpen.event,
    onDidCloseTextDocument: onDidClose.event,
    onDidChangeTextDocument: onDidChange.event,
    onDidSaveTextDocument: onDidSave.event,
    onWillSaveTextDocument: new EventEmitter().event,
    onDidChangeWorkspaceFolders: new EventEmitter().event,
    onDidCreateFiles: new EventEmitter().event,
    onDidDeleteFiles: new EventEmitter().event,
    onDidRenameFiles: new EventEmitter().event,
    async openTextDocument(arg) {
      if (arg && arg.content !== undefined) {
        return new TextDocumentImpl({ path: path.join(os.tmpdir(), `untitled-${Date.now()}`), languageId: arg.language, version: 1, text: arg.content });
      }
      const fsPath = typeof arg === 'string' ? arg : arg.fsPath;
      const open = documents.get(pathKey(fsPath));
      if (open) return open;
      const text = await fs.promises.readFile(fsPath, 'utf8');
      const doc = new TextDocumentImpl({ path: fsPath, languageId: 'plaintext', version: 1, text });
      documents.set(pathKey(fsPath), doc);
      return doc;
    },
    async applyEdit(edit) {
      const changes = edit.entries().map(([uri, edits]) => ({ path: uri.fsPath, edits: edits.map((e) => ({ range: [e.range.start.line, e.range.start.character, e.range.end.line, e.range.end.character], newText: e.newText })) }));
      try {
        return !!(await host.request('workspace.applyEdit', { changes }));
      } catch (error) {
        host.log('warn', `applyEdit failed: ${error.message}`);
        return false;
      }
    },
    fs: fsApi,
    findFiles,
    createFileSystemWatcher() {
      const never = new EventEmitter().event;
      return { onDidCreate: never, onDidChange: never, onDidDelete: never, ignoreCreateEvents: false, ignoreChangeEvents: false, ignoreDeleteEvents: false, dispose() {} };
    },
    registerTextDocumentContentProvider: noopDisposable,
    registerFileSystemProvider: noopDisposable,
    registerTaskProvider: noopDisposable,
    save: unsupported('workspace.save'),
    saveAll: () => Promise.resolve(true),
    updateWorkspaceFolders: () => false,
  };

  const languages = {
    createDiagnosticCollection: (name) => new DiagnosticCollection(name || `collection-${Date.now()}`),
    getLanguages: () => Promise.resolve(state.languages),
    getDiagnostics(uri) {
      return uri ? [] : [];
    },
    onDidChangeDiagnostics: new EventEmitter().event,
    setTextDocumentLanguage: unsupported('languages.setTextDocumentLanguage'),
    match(selector, document) {
      const list = Array.isArray(selector) ? selector : [selector];
      let best = 0;
      for (const s of list) {
        if (typeof s === 'string') best = Math.max(best, s === '*' ? 5 : s === document.languageId ? 10 : 0);
        else if (s) {
          if (s.language && s.language !== document.languageId) continue;
          if (s.scheme && s.scheme !== document.uri.scheme) continue;
          if (s.pattern && !globToRegExp(String(s.pattern.pattern || s.pattern)).test(document.fileName.replace(/\\/g, '/').split('/').pop())) continue;
          best = Math.max(best, 10);
        }
      }
      return best;
    },
  };
  for (const name of ['CompletionItemProvider', 'HoverProvider', 'DefinitionProvider', 'DeclarationProvider', 'TypeDefinitionProvider', 'ImplementationProvider', 'ReferenceProvider', 'DocumentHighlightProvider', 'DocumentSymbolProvider', 'WorkspaceSymbolProvider', 'CodeActionsProvider', 'CodeLensProvider', 'DocumentLinkProvider', 'ColorProvider', 'DocumentFormattingEditProvider', 'DocumentRangeFormattingEditProvider', 'OnTypeFormattingEditProvider', 'RenameProvider', 'SignatureHelpProvider', 'FoldingRangeProvider', 'SelectionRangeProvider', 'InlayHintsProvider', 'InlineCompletionItemProvider', 'CallHierarchyProvider', 'TypeHierarchyProvider', 'DocumentSemanticTokensProvider', 'DocumentRangeSemanticTokensProvider', 'EvaluatableExpressionProvider', 'InlineValuesProvider', 'LinkedEditingRangeProvider', 'DocumentDropEditProvider']) {
    languages[`register${name}`] = provider(`languages.register${name}`);
  }
  languages.registerCodeActionsProvider = provider('languages.registerCodeActionsProvider');

  const env = {
    appName: 'Forge',
    appRoot: path.dirname(process.execPath),
    appHost: 'desktop',
    language: 'en',
    machineId: state.machineId,
    sessionId: state.sessionId,
    uriScheme: 'forge',
    uiKind: enums.UIKind.Desktop,
    shell: process.env.ComSpec || process.env.SHELL || '',
    isTelemetryEnabled: false,
    isNewAppInstall: false,
    onDidChangeTelemetryEnabled: new EventEmitter().event,
    clipboard: { readText: () => Promise.resolve(''), writeText: () => Promise.resolve() },
    openExternal: (uri) => host.request('env.openExternal', { url: uri.toString() }).then(() => true, () => false),
    asExternalUri: (uri) => Promise.resolve(uri),
  };

  const extensions = {
    getExtension: extensionInfo,
    get all() { return state.extensions.map((e) => extensionInfo(e.id)); },
    onDidChange: new EventEmitter().event,
  };

  const l10n = { t: (message, ...args) => String(typeof message === 'object' ? message.message : message).replace(/\{(\d+)\}/g, (_, i) => String(args[i])), bundle: undefined, uri: undefined };

  const api = {
    version: '1.100.0',
    window,
    workspace,
    languages,
    commands,
    env,
    extensions,
    l10n,
    Uri, Position, Range, Selection, Disposable, EventEmitter, CancellationTokenSource, CancellationToken,
    Diagnostic, DiagnosticRelatedInformation, Location, TextEdit, WorkspaceEdit, MarkdownString, SnippetString,
    ThemeColor, ThemeIcon, Hover, CompletionItem, CompletionList, CodeAction, CodeLens, DocumentSymbol,
    SymbolInformation, FoldingRange, TreeItem, RelativePattern, FileSystemError,
    ...enums,
    // Namespaces Forge has no equivalent for: present so `vscode.debug` is an object, but unusable.
    debug: {},
    tasks: {},
    scm: {},
    notebooks: {},
    authentication: {},
    tests: {},
    chat: {},
    lm: {},
  };

  /** Called by the loader for each activated extension. */
  api.__createContext = function createContext(description, memento) {
    const root = description.root;
    return {
      subscriptions: [],
      extensionPath: root,
      extensionUri: Uri.file(root),
      extensionMode: enums.ExtensionMode.Production,
      environmentVariableCollection: { persistent: false, replace() {}, append() {}, prepend() {}, get() {}, forEach() {}, delete() {}, clear() {} },
      storageUri: Uri.file(path.join(state.storageDir, description.id, 'workspace')),
      storagePath: path.join(state.storageDir, description.id, 'workspace'),
      globalStorageUri: Uri.file(path.join(state.storageDir, description.id, 'global')),
      globalStoragePath: path.join(state.storageDir, description.id, 'global'),
      logUri: Uri.file(path.join(state.storageDir, description.id, 'logs')),
      logPath: path.join(state.storageDir, description.id, 'logs'),
      workspaceState: memento('workspace'),
      globalState: Object.assign(memento('global'), { setKeysForSync() {} }),
      secrets: (() => {
        const store = new Map();
        return { get: async (k) => store.get(k), store: async (k, v) => { store.set(k, v); }, delete: async (k) => { store.delete(k); }, onDidChange: new EventEmitter().event };
      })(),
      asAbsolutePath: (relative) => path.join(root, relative),
      extension: extensionInfo(description.id),
    };
  };

  api.__handlers = handlers;
  api.__executeCommand = (id, args) => {
    const handler = commandHandlers.get(id);
    if (!handler) throw new Error(`command '${id}' not found`);
    return handler(...(args || []));
  };
  api.__hasCommand = (id) => commandHandlers.has(id);

  // Unknown members fail loudly where they are used instead of silently being undefined.
  function guard(target, label) {
    return new Proxy(target, {
      get(object, prop, receiver) {
        if (prop in object || typeof prop === 'symbol' || prop === 'then' || prop === 'toJSON' || prop === 'inspect' || String(prop).startsWith('__')) return Reflect.get(object, prop, receiver);
        return unsupported(`${label}.${String(prop)}`);
      },
    });
  }
  for (const [name, ns] of Object.entries({ window, workspace, languages, commands, env, extensions })) api[name] = guard(ns, `vscode.${name}`);
  return guard(api, 'vscode');
};
