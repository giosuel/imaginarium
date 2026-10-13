# Test fonts

`EmojiRedSquare.ttf` is Noto Color Emoji 2.047 (the Debian/Ubuntu package
`fonts-noto-color-emoji`) cut down to one glyph, U+1F7E5 🟥 LARGE RED
SQUARE, a color bitmap (CBDT/CBLC, one strike at 109 pixels per em):

```sh
pyftsubset NotoColorEmoji.ttf --unicodes=U+1F7E5 --output-file=EmojiRedSquare.ttf
```

(fontTools 4.55.0.) It tests drawing bitmap glyphs, which vello_gpu can only
draw from its glyph atlas. License: SIL Open Font License 1.1, `OFL.txt`.
