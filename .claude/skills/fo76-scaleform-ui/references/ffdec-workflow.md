# FFDec workflow for Fallout 76 SWFs

Use this reference when JPEXS Free Flash Decompiler (FFDec) is the chosen static inspector or
patching tool. It distills safe static-inspection CLI mechanics and cross-checks command names against the
[FFDec command-line reference](https://github.com/jindrapetrik/jpexs-decompiler/wiki/Commandline-arguments).
It is a procedure for producing evidence, not a guarantee that FFDec's output will load in GFx.

## Scope and evidence boundary

- Pin the FFDec release, Java runtime, executable path, executable hash, target game build, target
  SWF/BA2 path, and input SHA-256 in an external research run.
- Keep originals, backups, exports, modified scripts, rebuilt SWFs, and visual previews outside
  this repository. Do not execute an SWF, installer, DLL, downloaded build script, or game binary.
- Treat decompiled ActionScript as an observation. Do not copy it into the repository, publish it,
  or treat it as an owned source tree.
- A successful FFDec export or replacement proves only that FFDec produced an output file. Runtime
  compatibility, permission to make a derivative, and permission to redistribute remain separate
  gates.

## Scratch run

Use a fresh run directory with a layout such as:

```text
run/
  input/       pristine SWF or verified nested payload
  backup/      byte-for-byte control copy
  reports/     dumps, link report, hashes, and manifests
  exports/     selected scripts and inspection assets
  candidate/   rebuilt output only
  verify/      post-patch exports and comparison reports
```

Set `FFDEC` to the pinned executable or wrapper for the current host. The official CLI supports
platform-specific launchers as well as `java -jar ffdec.jar`; do not assume the Windows path from
another skill works on Linux, macOS, or a different installation.

## Fingerprint before exporting

Start with bounded structural reports and only widen the scope when a report proves it necessary:

```sh
"$FFDEC" -dumpSWF run/input/target.swf > run/reports/tags.txt
"$FFDEC" -dumpAS3 run/input/target.swf > run/reports/classes.txt
"$FFDEC" -linkReport -outfile run/reports/link-report.xml run/input/target.swf
"$FFDEC" -export symbolClass run/reports/symbols run/input/target.swf
"$FFDEC" -swf2xml run/input/target.swf run/reports/target.xml
sha256sum run/input/target.swf > run/reports/input.sha256
```

Record the SWF signature/version/compression, frame rectangle, frame count/rate, ABC blocks,
classes, root symbol, symbol-class mappings, linkage names, and any nested `DefineBinaryData`
members. Export `binaryData` only when the target actually contains embedded data:

```sh
"$FFDEC" -export binaryData run/exports/binaryData run/input/target.swf
```

Use `frame`, `sprite`, `movie`, `image`, `shape`, `font`, or `text` exports only for a named
inspection question. Do not use `-export all` as a default shortcut: it expands the corpus,
creates rights obligations, and makes it easier to mistake decompiler output for source.

## Selective script inspection

Export only the allowlisted class or package needed for the hypothesis. FFDec's selection option
is an export pre-option and must appear before `-export script`:

```sh
"$FFDEC" -selectclass 'fully.qualified.TargetClass' \
  -export script run/exports/scripts run/input/target.swf

"$FFDEC" -selectclass 'fully.qualified.ui.+' \
  -export script run/exports/scripts run/input/target.swf
```

When readable ActionScript fails, preserve the failure report and use P-code only as a lower-level
inspection or patching representation:

```sh
"$FFDEC" -format script:pcode \
  -selectclass 'fully.qualified.TargetClass' \
  -export script run/exports/pcode run/input/target.swf
```

If recompilation needs broader context, export additional scripts into the external run directory
only. Do not copy the expanded decompile into this repository.

## Patch selection

Prefer a newly authored child widget or a minimal, target-specific ABC/bytecode edit. If a script
replacement is explicitly cleared and the compiler context is proven, FFDec supports a single
script replacement:

```sh
"$FFDEC" -replace run/input/target.swf run/candidate/target.swf \
  'fully.qualified.TargetClass' run/exports/edited/TargetClass.as
```

For a controlled batch import, pass the directory containing the expected `scripts/` tree, as
required by the installed FFDec version:

```sh
"$FFDEC" -importScript run/input/target.swf run/candidate/target.swf \
  run/exports/edited
```

Keep package wrappers, class names, linkage names, externally called method signatures, imports,
and required initialization paths unless the target evidence proves a change is safe. If control
flow skips initialization, audit every later dereference and cleanup path. Do not blindly apply the
source skill's browser-game examples: `ExternalInterface`, HTML callbacks, ad/loading bypasses,
CDN rewrites, and browser `eval` behavior are not Fallout 76 UI contracts.

For nested data, identify the actual member from the structural report and content hash. Never copy
an example character ID such as `3` into an FO76 command. Patch the verified nested SWF first, then
replace the outer member only if the outer container and derivative rights are both cleared:

```sh
"$FFDEC" -replace run/input/container.swf run/candidate/container.swf \
  '<verified-character-id>' run/candidate/nested-patched.swf
```

## Verify every candidate

Re-export the edited class and repeat the structural reports against the candidate:

```sh
"$FFDEC" -selectclass 'fully.qualified.TargetClass' \
  -export script run/verify/scripts run/candidate/target.swf
"$FFDEC" -dumpSWF run/candidate/target.swf > run/verify/tags.txt
"$FFDEC" -dumpAS3 run/candidate/target.swf > run/verify/classes.txt
"$FFDEC" -linkReport -outfile run/verify/link-report.xml run/candidate/target.swf
sha256sum run/candidate/target.swf > run/verify/candidate.sha256
```

Compare the intended class/resource and the complete structural report. A decompiler round-trip
need not reproduce identical text or constant-pool ordering; review semantic and packaging changes
instead. Confirm the candidate still has the expected signature/version/compression, frame data,
ABC structure, root/linkage symbols, required fonts/assets, and final `End` tag. Then run the
Scaleform skill's exact-build smoke tests for load, HUD mode, resolution, localization, focus,
input, reload/unload, and rollback. Label static results `[Confirmed]`, `[Deduced]`, or `[Unknown]`.

An FFDec warning such as experimental script replacement is not a pass. If the candidate cannot be
validated in the target game build, leave it as a research artifact and report the blocker.
