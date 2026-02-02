# podpull demo recording
# Run with: vhs demo.tape
#
# NOTE: This tape requires the parent directory (/Volumes/media/Podcasts) to
# exist on the system. The podcast output folder will be created by podpull.
# The sleep timeout after the command may need adjusting based on internet
# connectivity and episode file sizes.

Output demo.gif
Output demo.webm
Output demo.mp4

Set Shell "zsh"
Set FontFamily "JetBrainsMono NFM SemiBold"
Set FontSize 30
Set Width 1800
Set Height 800
# Custom theme matching landing page colors
Set Theme { "name": "LandingPage", "black": "#0f0f14", "red": "#ef6487", "green": "#5eca89", "yellow": "#fdd877", "blue": "#65aef7", "magenta": "#7c3aed", "cyan": "#43c1be", "white": "#e5e5e5", "brightBlack": "#9ca3af", "brightRed": "#ef6487", "brightGreen": "#5eca89", "brightYellow": "#fdd877", "brightBlue": "#65aef7", "brightMagenta": "#a78bfa", "brightCyan": "#43c1be", "brightWhite": "#ffffff", "background": "#0f0f14", "foreground": "#e5e5e5", "selection": "#2d2640", "cursor": "#7c3aed" }
Set TypingSpeed 40ms

Type "cd /Volumes/media/Podcasts"
Enter
Sleep 500ms

Type "podpull -l 3 https://14kltzj.podcaster.de/tad.rss './Trek am Dienstag – Der Star-Trek-Podcast'"
Enter

Sleep 33s
