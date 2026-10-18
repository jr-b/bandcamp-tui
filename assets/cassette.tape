# Demo GIF source file for the README. Record with: vhs cassette.tape
# https://github.com/charmbracelet/vhs
# Needs network and a release build (`cargo build --release`).

Output assets/demo.gif

Set Shell bash
Set FontSize 13
Set Width 1100
Set Height 620
Set Padding 10
Set Theme "Catppuccin Mocha"
Set TypingSpeed 40ms
Set Framerate 15
Set PlaybackSpeed 1.0

# Use a throwaway home so recording doesn't touch the real state.toml.
Hide
Type `export PATH="$PWD/target/release:$PATH" HOME=$(mktemp -d) PS1="$ " && clear` Enter
Show

Type "bandcamp-tui -t ambient -s top --no-art"
Sleep 300ms
Enter
Sleep 3s

# Browse the Discover list; the album pane follows the selection.
Down@450ms 4
Sleep 1.5s

# Album pane: tracklist, then about / credits.
Tab
Sleep 800ms
Down@400ms 3
Sleep 800ms
Type "i"
Sleep 2s
Type "i"
Sleep 600ms
Tab
Sleep 800ms

# Switch genre.
Type "f"
Sleep 800ms
Type@90ms "jazz"
Sleep 1s
Enter
Sleep 3.5s
Down@450ms 3
Sleep 1.5s

# Play, then the listening view.
Enter
Sleep 3s
Type "v"
Sleep 3s
Type "i"
Sleep 3s
Type "i"
Sleep 1.5s
Type "q"
Sleep 300ms
