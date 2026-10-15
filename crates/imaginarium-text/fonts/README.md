# Bundled fonts

- `InterVariable.ttf`: [Inter](https://github.com/rsms/inter) 4.001,
  variable (weight 100–900, optical size), by The Inter Project Authors.
  Licensed under the SIL Open Font License 1.1 (`OFL.txt`), not under the
  AGPL like the code around it. The OFL allows bundling it with software.

It is the UI font of the gate app and the font of the text tests, so
layouts and golden images are the same on every machine. Other scripts
(CJK, Arabic, emoji, …) fall back to the system's fonts.
