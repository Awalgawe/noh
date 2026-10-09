# Restore optional Windows content delivery

## Verified recovery inventory

The public root `741da7a` already contains `tools/setup_profiles.rs`,
`tools/build-installer.ps1`, `tools/installer/noh.iss` and
`src/update/bootstrap.rs`. They were not deleted by the public import.
Commit `522d91b` introduced the three profiles and authenticated web installer;
`1b7536e` repaired download cancellation. The public release added a separate
ordinary installer (`public.iss`) without connecting those profile capabilities.
The qualified application was built with `gui,mcp`, without `updates` or the
private Setup guardian/repair executables. Restoring the old workflow alone
would therefore not make that installation engine available in the public app.

The additional object-composition experiment remains on
`codex/native-component-qualification`, with `tools/payload_components.py`,
`tools/setup_component_kit.py`, `tools/setup_components.rs` and the final wrapper
qualification at `b3cc93a`. Its report explicitly rejects general adoption as a
build acceleration technique. The native helper reconstructs signed Setup bytes;
it does not download individual media tools into a running NOH installation.
Do not merge the experimental branch wholesale or present that experiment as a
previously shipped on-demand component manager.

`src/resources.rs` and `src/app_resources.rs` already detect missing FFmpeg,
speech/model files and preview support, show translated recovery help, and
recheck availability off the GUI thread. The inspected history contains no
FFmpeg/Whisper download action there. The public app currently links to the
Microsoft prerequisite download. `src/app_updates.rs` provides a separate,
feature-gated full-application update flow, not media-component acquisition.

## Current user requirement

Provide both content selection/download in the minimal installer and an action
inside NOH when a feature needs missing content. Preserve the earlier three
offline profiles and small web entrypoint. Existing installations must be able
to add content without uninstalling. Keep the seven languages, progress,
cancellation, user files, exact source identity and redistribution constraints.
Remove obsolete workflows only after their required capabilities have a route
through the canonical Release workflow.

## Integration decision for review

Reuse the existing profile boundaries: application; FFmpeg plus preview; speech
runtime plus models. Reuse the resource diagnostics and recheck behavior in the
app, and the established Inno profile presentation and download controls. Keep
the current public per-user installation format and its ordinary uninstaller.
Do not restore the private updater merely to acquire optional media components.

Create two versioned, hash-pinned component archives from the already qualified
native payload: media and speech. Preserve complete dependency groups, notices
and corresponding sources. Speech also requires media. Both setup-time and
later additions must consume these same archives and the same installation
code, instead of implementing two independent download/extraction engines.

Use a small Inno maintenance helper shipped beside the app. The minimal, standard
and complete wrappers embed their default content and offer additional content
from the official release. The helper offers additions to an existing compatible
installation; the GUI opens this helper from the missing-resource panel. Native
Inno progress/cancellation handles transfer without blocking the GUI. This
avoids adding another Rust HTTP/archive stack when the already used installer
can perform the same operation. Restrict the helper to the matching installed
application and known content; verify archive size/digest before extraction.

Preserve installed content on repair and reject downgrades or incompatible app
identities. Register additions with the same uninstaller. An ordinary repair
must not remove tools acquired later. Establish the exact Inno registration,
profile persistence and cancellation behavior through native tests; the first
local prototype exposed that writing a custom value into the uninstall key in
the Registry section does not survive Inno's subsequent registration step.
That prototype is not approved or published.

The GUI action is an application change, so prepare a new patch release rather
than changing the application bytes attached to v0.1.0. Compile the application
once for that release; derive all content profiles from it and reuse unchanged
native tools/models. Keep wrapper provenance separate from application identity.

## Acceptance boundary

Before publication, validate installation of each profile, selecting additional
content during minimal setup, adding media and then speech after installation,
unchanged application/user files, repair retention and complete uninstall.
Exercise real download, hash rejection, network failure, cancellation and retry.
Exercise the GUI action and subsequent resource detection on the exact release
bytes, with translated UI checks. Record when a restart is needed for preview.
Recheck legal/source correspondence for the unchanged native payload and the new
application build. Do not claim the old private qualification covers new public
installers or the new GUI entrypoint.

The current uncommitted workflow/profile implementation remains a prototype.
No workflow removal, application release or new installer has been published.
