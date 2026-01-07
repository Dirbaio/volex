import * as vscode from "vscode";
import {
  LanguageClient,
  LanguageClientOptions,
  ServerOptions,
} from "vscode-languageclient/node";

let client: LanguageClient | undefined;
let outputChannel: vscode.OutputChannel | undefined;

function startLanguageServer() {
  if (!outputChannel) {
    outputChannel = vscode.window.createOutputChannel("Volex Language Server");
  }

  // Get the LSP server path from configuration or use PATH
  const config = vscode.workspace.getConfiguration("volex");
  const serverPath = config.get<string>("lspPath") || "volex-lsp";

  outputChannel.appendLine(`Starting Volex Language Server...`);
  outputChannel.appendLine(`Using LSP server at: ${serverPath}`);

  // Define the server options
  const serverOptions: ServerOptions = {
    command: serverPath,
    args: [],
    options: {
      env: {
        ...process.env,
        RUST_LOG: "info",
      },
    },
  };

  // Define the client options
  const clientOptions: LanguageClientOptions = {
    documentSelector: [{ scheme: "file", language: "volex" }],
    synchronize: {
      fileEvents: vscode.workspace.createFileSystemWatcher("**/*.vol"),
    },
    outputChannel: outputChannel,
  };

  // Create and start the language client
  client = new LanguageClient(
    "volexLanguageServer",
    "Volex Language Server",
    serverOptions,
    clientOptions
  );

  // Start the client
  client.start().catch((error) => {
    outputChannel?.appendLine(`Failed to start language server: ${error}`);
    vscode.window.showErrorMessage(
      `Failed to start Volex Language Server: ${error.message}`
    );
  });

  outputChannel.appendLine(`Language server started.`);
}

export function activate(context: vscode.ExtensionContext) {
  outputChannel = vscode.window.createOutputChannel("Volex Language Server");
  outputChannel.appendLine("Volex extension is now active");

  startLanguageServer();

  // Register restart command
  const restartCommand = vscode.commands.registerCommand(
    "volex.restartServer",
    async () => {
      if (client) {
        await client.stop();
      }
      startLanguageServer();
      vscode.window.showInformationMessage("Volex language server restarted");
    }
  );

  context.subscriptions.push(restartCommand);
}

export function deactivate(): Thenable<void> | undefined {
  if (!client) {
    return undefined;
  }
  return client.stop();
}
