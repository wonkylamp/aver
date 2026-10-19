# Aver on Windows (WSL2)

Aver runs on Windows inside WSL2, the Linux environment that ships with Windows 10 (version 2004 or later) and Windows 11. Native Windows is not supported: the install script refuses to run in PowerShell, Git Bash, MSYS2 or Cygwin. Everything below happens in the WSL2 Linux shell unless a step says otherwise.

## 1. Install WSL2 with Ubuntu

In PowerShell as administrator:

```powershell
wsl --install -d Ubuntu
```

Restart when asked, then open Ubuntu from the Start menu and create your Linux user. Check that it runs WSL version 2:

```powershell
wsl -l -v
```

## 2. The project's toolchain

Aver runs checks natively inside Ubuntu, with the project's own toolchain (Node, Python, Rust or Go). Install it in Ubuntu as you would to run the project's tests there. Aver does not use Docker.

## 3. Git and Node.js

```bash
sudo apt update && sudo apt install -y git curl
```

Node projects need Node.js, and so do agent CLIs installed with `npm`. One way to install Node.js 22:

```bash
curl -fsSL https://deb.nodesource.com/setup_22.x | sudo -E bash -
sudo apt install -y nodejs
node --version
```

## 4. Install Aver

```bash
curl -fsSL https://raw.githubusercontent.com/wonkylamp/aver/main/site/install.sh | sh
```

It installs `aver` to `~/.local/bin`. If the script says that directory is not on your `PATH`, run the line it prints and open a new shell.

## 5. Keep repositories in the Linux filesystem

Clone into your Linux home, for example `~/code`, not under `/mnt/c`:

```bash
mkdir -p ~/code && cd ~/code
git clone https://github.com/<owner>/<repo>.git
```

Files under `/mnt/c` cross the Windows filesystem bridge. Workspace creation, file scans and `npm` installs are much slower there, and copy-on-write clones are not available. From Windows, the Linux files are at `\\wsl$\Ubuntu\home\<you>\code`.

## 6. Agent tools

Run the agent where the repository is.

- **Claude Code, Codex, Gemini CLI**: install and run them inside Ubuntu, not in PowerShell. For example `npm install -g @openai/codex` or `npm install -g @google/gemini-cli`; install Claude Code with its Linux instructions. Sign in from the Ubuntu shell.
- **VS Code**: install the WSL extension on Windows, then run `code .` in the repository in Ubuntu. VS Code opens a remote window whose terminal and extensions run in Linux.
- **Cursor**: connect to WSL from Cursor's remote menu and open the folder under `~/code`.

## 7. Connect and check

In the repository, inside Ubuntu:

```bash
aver setup
aver doctor
```

`aver setup` detects your agent tools and writes their project-local configuration after one question. `aver doctor` checks Git, the toolchains, the agent CLIs and the repository, and prints a fix for each problem.
