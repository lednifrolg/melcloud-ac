# The shell screenshot in the README; see the README's development section.

Output "assets/shell.mp4"

Set Shell "bash"
Set FontFamily "JetBrainsMono Nerd Font"
Set FontSize 14
Set Width 860
Set Height 600
Set Padding 16
Set Theme "Catppuccin Mocha"
Set TypingSpeed 40ms
Set Framerate 30

Hide
Type "export KLIMA_HOME=$(mktemp -d) KLIMA_API_URL=http://127.0.0.1:18893 PATH=$PWD/target/debug:$PATH"
Enter
Type "target/debug/examples/demo_server 18893 $KLIMA_HOME > /dev/null & sleep 1"
Enter
Type "echo demo | klima login --email user@example.com --password-stdin > /dev/null"
Enter
Type "PS1='\[\e[1;34m\]❯\[\e[0m\] '; clear"
Enter
Sleep 1s
Show

Type "klima status"
Enter
Sleep 1s
Type "klima temp 23"
Enter
Sleep 4s
Type "klima history"
Enter
Sleep 3s

Hide
Type "kill %1"
Enter
