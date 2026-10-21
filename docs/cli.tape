# Renders docs/cli.gif with https://github.com/charmbracelet/vhs.
# Needs an Arc on USB and a release build: cargo build --release -p otii-cli
# Run from the repository root: vhs docs/cli.tape

Output docs/cli.gif
Require openotii

Set Shell bash
Set FontFamily "JetBrains Mono"
Set FontSize 15
Set Width 940
Set Height 440
Set Padding 24
Set TypingSpeed 35ms
Set Theme { "name": "zinc", "background": "#09090b", "foreground": "#e4e4e7", "cursor": "#e4e4e7", "selection": "#3f3f46", "black": "#27272a", "brightBlack": "#71717a", "red": "#f87171", "brightRed": "#fca5a5", "green": "#4ade80", "brightGreen": "#86efac", "yellow": "#facc15", "brightYellow": "#fde047", "blue": "#60a5fa", "brightBlue": "#93c5fd", "magenta": "#c084fc", "brightMagenta": "#d8b4fe", "cyan": "#22d3ee", "brightCyan": "#67e8f9", "white": "#d4d4d8", "brightWhite": "#fafafa" }

Hide
Type "export PATH=$PWD/target/release:$PATH PS1='\[\e[2m\]$\[\e[0m\] ' && clear"
Enter
Show

Type "openotii info"
Sleep 300ms
Enter
Sleep 2s

Type "openotii power --volt 1.8V --ilim 50mA on"
Sleep 300ms
Enter
Sleep 1.5s

Type "openotii measure --ch mc,mv,mp --duration 3s --assert 'mc.mean<20mA'"
Sleep 300ms
Enter
Sleep 4.5s
Type "echo $?"
Enter
Sleep 2s

Ctrl+L
Type "openotii measure --ch mc --duration 1s --assert 'mc.mean<10mA'"
Sleep 300ms
Enter
Sleep 2.5s
Type "echo $?"
Enter
Sleep 1.5s

Type "openotii power off"
Sleep 300ms
Enter
Sleep 3s
