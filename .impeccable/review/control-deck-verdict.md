# Control deck finish review

Disposition: GTK workflow editor approved after one material-fix batch. GNOME
Shell changes remain source/syntax reviewed; rendered Shell approval requires
the user's next login. No Shell restart or logout was forced.

Resolved: visible selected library row; in-session drafts survive workflow
switching and search; Run is disabled and guarded for unsaved changes; the
internal Hidden state is presented as Ready. Native GTK regression coverage
exercises these draft and selection behaviors.

The final `workflows-desktop.png` is an actual GTK widget render at 1120×760.
Its workflows come from an illustrative temporary configuration, not the user's
configuration. The final capture uses an isolated runtime directory so its
footer does not mix sample workflows with live service feedback. It is not
evidence that the illustrated workflows were executed.

The existing butterfly asset was reused; no new shipping raster asset was
generated. The user-supplied modular-device image guided the graphite rail,
blue transport controls, and numbered category-colored modules. Native GTK
controls, typography, and themed editor surfaces were retained.

Validation: laptop/workflow unit suites; graphical draft-preservation test;
live local CDP page read/fill/click test in the dedicated Brave Flatpak profile;
extension JavaScript and installer shell syntax checks. Package-scoped Clippy
passes with the existing large-error lint exception. Workspace-wide Clippy has
unrelated warnings in sibling config, model, and protocol crates.
