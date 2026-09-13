# The code rain's font

`NotoSansCJKjp-DemiLight-Rain.otf` is Noto Sans CJK JP DemiLight (SIL Open Font License 1.1, in
`NotoSansCJK-OFL.txt`), cut down to what the rain draws: ASCII for app names, and half-width
katakana with the digits for the rain itself. It was made in the `slipstream` distrobox with
fonttools:

```sh
python3 -m venv ~/.cache/slipstream-tools/venv
~/.cache/slipstream-tools/venv/bin/pip install fonttools
~/.cache/slipstream-tools/venv/bin/pyftsubset \
  /usr/local/share/fonts/google-noto-sans-cjk-fonts/NotoSansCJK-DemiLight.ttc --font-number=0 \
  --unicodes="U+0020-007E,U+FF61-FF9F" --layout-features="" --no-hinting --desubroutinize \
  --name-IDs="*" --output-file=assets/fonts/NotoSansCJKjp-DemiLight-Rain.otf
```
