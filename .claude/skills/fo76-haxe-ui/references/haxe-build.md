# Haxe build reference for Fallout 76 UI

This reference covers the Haxe compiler features that are useful for a small Fallout 76 SWF. It
does not claim that Haxe's Flash target or OpenFL's Flash/AIR output is a supported Bethesda SDK.
Read it with `fo76-scaleform-ui` and the FFDec workflow.

## Toolchain discovery

Record the output of the host equivalents of:

```sh
haxe --version
haxe --help
haxelib list
java -version
```

Do not initialize a global Haxelib directory or install a library as an incidental step in a
research run. Make dependency installation explicit, pinned, reviewable, and outside the game
payload. A missing Haxelib/OpenFL installation is a reported capability gap, not a reason to fetch
unrelated packages automatically.

The Haxe compiler exposes a Flash/SWF target, `--swf`, `--swf-version`, `--swf-lib`,
`--swf-lib-extern`, `--dce`, and `--no-output`, plus Flash defines such as `flash-strict` and
`swf-header`. The target flags are compiler features; choose their values from the active FO76
artifact and runtime evidence. Recent compilers may warn that the long `--flash-strict` and
`--swf-header` spellings are deprecated, so prefer `-D flash-strict` and `-D swf-header=...` in
reproducible HXML profiles.

## Suggested source layout

Keep an authoring project outside the repository's game/mod research corpus when it contains
payloads or private assets:

```text
widget-project/
  src/              Haxe implementation
  externs/          proven native/SWC declarations
  fixtures/         synthetic fonts, icons, and bridge responses
  reports/          compiler and SWF inspection reports
  build/            generated SWFs, never committed here
  check.hxml        type-check profile
  widget.hxml       candidate SWF profile
```

The repository may contain the Haxe source and synthetic fixtures when their rights are clear. Keep
game-derived inputs, rebuilt payloads, and decompiler output in an external run root.

## Check and build profiles

Use a check profile that compiles the same source graph without writing a SWF:

```text
--swf build/check.swf
--no-output
--class-path src
--class-path externs
--main Main
-D flash-strict
--dce no
```

Use a candidate profile only after the target header is recorded:

```text
--swf build/widget.swf
--class-path src
--class-path externs
--main Main
--swf-version <proven-version>
-D swf-header=<width>:<height>:<fps>:<background>
-D flash-strict
--dce no
```

During ABI bring-up, `--dce no` makes reachability easier to inspect. After the entrypoint and
linkage contract is proven, measure whether standard DCE can be enabled and retain only the
required roots. Do not claim that a smaller SWF is safer without inspecting its symbols and
runtime behavior.

After every build, inspect the emitted header instead of trusting the requested compiler values.
Compiler revisions may normalize or clamp SWF version/header settings, and a successful Haxe
command does not prove that the resulting signature, version, frame rectangle, or compression is
accepted by Fallout 76 GFx.

If a cleared SWC supplies types but must not be packaged, use `--swf-lib-extern <file>`. Use
`--swf-lib <file>` only when the SWC is an intentional, rights-cleared runtime dependency. Inspect
the resulting ABC and link report after either choice.

## Externs and native names

Externs provide compile-time types for APIs that already exist at runtime; they do not load a DLL,
create a GFx callback, or grant access to an extender. Keep the declaration narrow and versioned:

```haxe
#if swf
@:native("ProvenNativeSurface")
extern class ProvenNativeSurface {
    static function call(command:String, payloadJson:String):String;
}
#end
```

Replace `ProvenNativeSurface` and its signature only with a name and contract positively identified
in the target artifact/provider documentation. Do not use this pattern to guess `__SFCodeObj`, ZFE,
xScal, or HUDModLoader behavior. Keep provider handshakes, JSON validation, error handling, and
fallback behavior in ordinary authored code around the extern.

Use target-specific files or `#if swf` to keep desktop/unit-test seams separate from the game movie.
Do not let a desktop mock silently stand in for GFx. A test that uses `--interp`, Neko, or a desktop
preview can check pure state/timing logic; only an exact-build smoke test can confirm the bridge,
fonts, input, focus, and rendering contract.

## Asset metadata

Haxe Flash metadata such as `@:bitmap`, `@:file`, and `@:font` can embed files in a generated SWF.
Use them only for authored or separately cleared files. A successful embed does not prove that the
FO76 application domain resolves the font, texture format, linkage, or symbol name. Record the
embedded asset hash and verify it in the candidate SWF.

## Haxe-specific failure modes

- **Compiles but no widget appears:** check the root class, linkage/export name, loader entry, BA2
  path, and load order before changing code.
- **Bridge call is undefined:** an extern only affects compilation; re-check the documented provider
  handshake and application-domain name.
- **Symbols disappear:** inspect DCE and explicit roots; do not solve this with broad reflection.
- **FFDec cannot recompile the class:** retain the pristine candidate, export P-code for evidence,
  and switch to a minimal bytecode/ABC patch or newly authored child movie.
- **Desktop preview differs from game:** record it as preview evidence only and run the exact-build
  smoke gate; do not add unsupported filters, shaders, or framework assumptions.

Useful primary references are the [Haxe Flash target guide](https://haxe.org/manual/target-flash.html),
[Haxe externs](https://haxe.org/manual/lf-externs.html),
[Flash target metadata](https://haxe.org/manual/target-flash-metadata.html), and the
[OpenFL SWF asset guide](https://www.openfl.org/learn/haxelib/tutorials/using-swf-assets/).
