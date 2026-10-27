# The README's demo and screenshots, recorded with vhs (https://github.com/charmbracelet/vhs)
# against the demo server in crates/klima/examples, so no request reaches MELCloud:
#
#   cargo build -p klima --examples
#   vhs assets/demo.tape
#
# vhs's Screenshot command misses the TUI's alternate screen, so the screenshots are frames of
# assets/demo.mp4, picked with ffmpeg (see the README's development section).

Output "assets/demo.gif"
Output "assets/demo.mp4"

Set Shell "bash"
Set FontFamily "JetBrainsMono Nerd Font"
Set FontSize 14
Set Width 1040
Set Height 640
Set Padding 16
Set Theme "Catppuccin Mocha"
Set TypingSpeed 60ms
Set Framerate 30

Hide
Type "export KLIMA_HOME=$(mktemp -d) KLIMA_API_URL=http://127.0.0.1:18891 PATH=$PWD/target/debug:$PATH"
Enter
Type "target/debug/examples/demo_server 18891 $KLIMA_HOME > /dev/null & sleep 1"
Enter
Type "echo demo | klima login --email user@example.com --password-stdin > /dev/null"
Enter
Type "PS1='\[\e[1;34m\]❯\[\e[0m\] '; clear"
Enter
Sleep 1s
Show

Type "klima status"
Sleep 300ms
Enter
Sleep 2.5s

Type "klima"
Sleep 300ms
Enter
Sleep 3s
Type "+"
Sleep 400ms
Type "+"
Sleep 1.2s
Enter
Sleep 3.5s
Type "m"
Sleep 400ms
Type "m"
Sleep 1.2s
Enter
Sleep 4s
Type "?"
Sleep 2.5s
Type "?"
Sleep 500ms
Type "q"
Sleep 500ms

Type "klima history"
Sleep 300ms
Enter
Sleep 4s

Hide
Type "kill %1"
Enter
