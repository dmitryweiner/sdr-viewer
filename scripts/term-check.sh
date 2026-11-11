#!/bin/sh
# Shows whether this terminal can draw what SDR Viewer needs: truecolor,
# the eight bar heights, half blocks (two waterfall pixels per cell) and
# Braille dots. Run it in the terminal the app will run in (phase 0).
#
#   ./scripts/term-check.sh
#
# Look at the output: the gradient must be smooth (not banded into ~16
# steps), and no glyph may show as a box or a question mark.
printf 'Size: %s columns x %s rows (the app wants 80x24 at least, ~160x45 is the design)\n' \
  "$(tput cols)" "$(tput lines)"
printf 'TERM=%s COLORTERM=%s\n\n' "$TERM" "${COLORTERM:-<unset>}"

printf 'Truecolor gradient (half blocks, top and bottom colours differ):\n'
i=0
while [ $i -lt 64 ]; do
  r=$((i * 4)); b=$((255 - i * 4))
  printf '\033[38;2;%d;0;%dm\033[48;2;0;%d;%dm▀' $r $b $r $b
  i=$((i + 1))
done
printf '\033[0m\n\n'

printf 'Bars:    ▁▂▃▄▅▆▇█  ▁▃▅▇█▇▅▃▁\n'
printf 'Blocks:  ▀ ▄ █ ░ ▒ ▓\n'
printf 'Braille: ⠁⠂⠄⡀⠈⠐⠠⢀ ⣀⣤⣶⣿ ⢸⡇\n'
printf 'Strip:   ···▂██▃····▅█··\n'
printf 'Lines:   ┌─┬─┐ │ ├─┼─┤ └─┴─┘ │ ▶ ◀ ▲ ▼ •\n'
