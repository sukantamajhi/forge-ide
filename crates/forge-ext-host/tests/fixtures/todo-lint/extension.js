// A small extension for testing Forge's extension host: it flags a marker word, keeps a
// status bar count, and offers commands. It uses only the public `vscode` API.
const vscode = require('vscode');

function activate(context) {
  const diagnostics = vscode.languages.createDiagnosticCollection('todo-lint');
  const output = vscode.window.createOutputChannel('Todo Lint');
  const status = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Right, 100);
  status.command = 'todoLint.count';
  status.show();
  context.subscriptions.push(diagnostics, output, status);

  function word() {
    return vscode.workspace.getConfiguration('todoLint').get('word', 'TODO');
  }

  function lint(document) {
    const marker = word();
    const found = [];
    for (let line = 0; line < document.lineCount; line++) {
      const text = document.lineAt(line).text;
      let at = text.indexOf(marker);
      while (at !== -1) {
        const range = new vscode.Range(line, at, line, at + marker.length);
        const d = new vscode.Diagnostic(range, `Unfinished work: ${marker}`, vscode.DiagnosticSeverity.Warning);
        d.source = 'todo-lint';
        found.push(d);
        at = text.indexOf(marker, at + marker.length);
      }
    }
    diagnostics.set(document.uri, found);
    if (vscode.window.activeTextEditor && vscode.window.activeTextEditor.document === document) {
      status.text = `$(checklist) ${found.length} ${marker}${found.length === 1 ? '' : 's'}`;
    }
    return found.length;
  }

  context.subscriptions.push(
    vscode.workspace.onDidOpenTextDocument(lint),
    vscode.workspace.onDidChangeTextDocument((e) => lint(e.document)),
    vscode.workspace.onDidCloseTextDocument((d) => diagnostics.delete(d.uri)),
    vscode.window.onDidChangeActiveTextEditor((editor) => editor && lint(editor.document)),
    vscode.workspace.onDidChangeConfiguration(() => vscode.workspace.textDocuments.forEach(lint)),
    vscode.commands.registerCommand('todoLint.hello', () => {
      vscode.window.showInformationMessage('Hello from Todo Lint');
      return 'hello';
    }),
    vscode.commands.registerCommand('todoLint.count', () => {
      const editor = vscode.window.activeTextEditor;
      const count = editor ? lint(editor.document) : 0;
      output.appendLine(`${count} marker(s) in ${editor ? editor.document.fileName : 'no file'}`);
      return count;
    }),
    vscode.commands.registerCommand('todoLint.fix', async () => {
      const editor = vscode.window.activeTextEditor;
      if (!editor) return false;
      const marker = word();
      const replacement = vscode.workspace.getConfiguration('todoLint').get('fixWith', 'DONE');
      const edit = new vscode.WorkspaceEdit();
      for (let line = 0; line < editor.document.lineCount; line++) {
        const text = editor.document.lineAt(line).text;
        const at = text.indexOf(marker);
        if (at !== -1) edit.replace(editor.document.uri, new vscode.Range(line, at, line, at + marker.length), replacement);
      }
      return vscode.workspace.applyEdit(edit);
    }),
  );

  // State that survives restarts.
  const runs = (context.globalState.get('runs') || 0) + 1;
  context.globalState.update('runs', runs);
  output.appendLine(`activated (run ${runs})`);
  vscode.workspace.textDocuments.forEach(lint);
  return { lint: (text) => text.split('\n').filter((l) => l.includes(word())).length };
}

function deactivate() {}

module.exports = { activate, deactivate };
