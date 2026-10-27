# The narrow layout for the README's screenshots; see demo.tape.

Output "assets/narrow.mp4"

Set Shell "bash"
Set FontFamily "JetBrainsMono Nerd Font"
Set FontSize 14
Set Width 660
Set Height 500
Set Padding 16
Set Theme "Catppuccin Mocha"
Set Framerate 30

Hide
Type "export KLIMA_HOME=$(mktemp -d) KLIMA_API_URL=http://127.0.0.1:18892 PATH=$PWD/target/debug:$PATH"
Enter
Type "target/debug/examples/demo_server 18892 $KLIMA_HOME > /dev/null & sleep 1"
Enter
Type "echo demo | klima login --email user@example.com --password-stdin > /dev/null"
Enter
Type "clear; klima"
Enter
Sleep 2s
Show

Sleep 2s
Type "3"
Sleep 2s
Type "1"
Sleep 2s

Hide
Type "q"
Type "kill %1"
Enter
