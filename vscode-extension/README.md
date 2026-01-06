# Volex VSCode Extension

This extension provides language support for Volex schema files (`.vol`).

## Features

- **Syntax Highlighting**: Full syntax highlighting for Volex language constructs (struct, message, enum, union, service)
- **Diagnostics**: Real-time error checking and validation
- **Go to Definition**: Navigate to type definitions with `F12` or Ctrl+Click
- **Hover Information**: View type information by hovering over identifiers

## Setup

1. Build the LSP server:

   ```bash
   cargo build --release --bin volex-lsp
   ```

2. Install extension dependencies:

   ```bash
   cd vscode-extension
   npm install
   ```

3. Compile the extension:

   ```bash
   npm run compile
   ```

4. Install the extension in VSCode:
   - Press `F5` in VSCode to launch the extension development host
   - Or package and install: `vsce package` then install the `.vsix` file

## Configuration

You can configure the path to the LSP server in VSCode settings:

```json
{
  "volex.lspPath": "/path/to/volex-lsp"
}
```

If not specified, the extension will look for `volex-lsp` in:

1. `target/debug/volex-lsp` in the workspace
2. `target/release/volex-lsp` in the workspace
3. `volex-lsp` in your PATH

## Development

To work on the extension:

1. Open the `vscode-extension` folder in VSCode
2. Run `npm install`
3. Press `F5` to launch the extension development host
4. Make changes and reload the window to test
