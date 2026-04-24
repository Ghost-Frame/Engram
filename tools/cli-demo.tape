# VHS tape: renders tools/cli-demo.gif for the project README.
#
# Prerequisites:
#   pacman -S vhs ttyd ffmpeg
#   cargo build --release -p kleos-cli --bin kleos-cli
#
# Render (needs a running Kleos server + valid API key in env):
#   export KLEOS_URL=http://<host>:4200
#   export KLEOS_API_KEY=<key>
#   cd tools && vhs cli-demo.tape

Output cli-demo.gif

Set Shell "bash"
Set FontSize 16
Set Width 900
Set Height 520
Set Theme "TokyoNight"
Set TypingSpeed 55ms
Set PlaybackSpeed 1.0

# Isolate the recording from the user's rc files / local aliases.
Hide
Type `export PS1='$ '` Enter
Type `alias kleos-cli="$(pwd)/../target/release/kleos-cli"` Enter
Type `clear` Enter
Show

Sleep 400ms
Type "kleos-cli store 'Kleos is the Rust-native memory system for AI agents.' -c general" Sleep 300ms Enter
Sleep 1800ms

Type "kleos-cli search 'memory system' --limit 3" Sleep 300ms Enter
Sleep 2600ms

Type "kleos-cli context 'what is kleos' --limit 3" Sleep 300ms Enter
Sleep 2800ms

Type "kleos-cli list --limit 5" Sleep 300ms Enter
Sleep 2600ms

Sleep 1200ms
