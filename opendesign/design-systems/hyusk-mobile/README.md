# Hyusk mobile

A compact design system extracted from the Android implementation and the Hyusk butterfly brand asset.

Sources consulted:

- `android/app/src/main/java/io/github/hyusk/mobile/ui/VoiceHomeScreen.kt`
- `android/app/src/main/java/io/github/hyusk/mobile/MainActivity.kt`
- `android/app/src/main/java/io/github/hyusk/mobile/services/HyuskVoiceInteractionService.kt`
- `android/app/src/main/res/drawable-nodpi/hyusk_butterfly_mark.png`

## Foundations

- Ink is the dominant surface; pearl is the dominant content color.
- Cyan indicates live listening or activity, never decoration.
- The butterfly is centered and breathes with voice level. Avoid enclosing it in a large colored sphere.
- Panels use 32 px radii; interactive rows use 24 px radii; borders are one quiet graphite line.
- Use 240 ms state transitions and one 1350 ms breathing cycle.
- Use sentence case for controls and responses. `HYUSK` may use tracked capitals as a brand label.
- Do not use emoji as icons. Use the shipped butterfly asset or platform vector icons.

The native assistant overlay and full voice screen should share these rules while differing in density.
