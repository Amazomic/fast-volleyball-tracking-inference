# Rust kernels for Volleyball Studio

Stage S01: pure, deterministic CPU geometry with an opt-in ctypes bridge. The Python path remains authoritative by default; no GPU / FFmpeg backend has been migrated.

From the repository root:

```powershell
& "$env:USERPROFILE\.cargo\bin\cargo.exe" fmt --manifest-path rust/volley-core/Cargo.toml -- --check
& "$env:USERPROFILE\.cargo\bin\cargo.exe" test --manifest-path rust/volley-core/Cargo.toml
& "$env:USERPROFILE\.cargo\bin\cargo.exe" build --manifest-path rust/volley-core/Cargo.toml
$env:VOLLEY_RUST_REQUIRED = '1'
python -m unittest discover -s tests -p test_rust_core_contract.py
Remove-Item Env:VOLLEY_RUST_REQUIRED
```

To opt in to Rust IoU during CPU player replay **only after parity verification**:

```powershell
$env:VOLLEY_RUST_IOU = '1'
```

Windows build target: `x86_64-pc-windows-msvc`; opt-in requires **64-bit Python**. The workstation also has a 32-bit Python 3.14 interpreter, which cannot load this DLL. That interpreter gets a clear error if Rust is explicitly enabled. If Rust is explicitly requested but the DLL is absent, the adapter errors instead of silently using Python. Remove `VOLLEY_RUST_IOU` for the previous Python implementation. Build products under `rust/volley-core/target/` are ignored; project and GPU caches are untouched.

## S02a: batched ball-motion confirmation (opt-in)

The Rust `motion` module reproduces the gravity-corrected velocity-break evidence used by RAVEL Touch postprocessing. One C ABI call evaluates all proposed Touch frames against the same saved trajectory. The Python threshold constants are passed at runtime; no model is rerun or cache changed.

Build and compare both implementations:

```powershell
& "$env:USERPROFILE\.cargo\bin\cargo.exe" test --manifest-path rust/volley-core/Cargo.toml
& "$env:USERPROFILE\.cargo\bin\cargo.exe" build --manifest-path rust/volley-core/Cargo.toml
$env:VOLLEY_RUST_REQUIRED = '1'
python -m unittest discover -s tests -p test_rust_motion_contract.py
$env:VOLLEY_RUST_MOTION = '1'
python -m unittest discover -s tests -p test_ravel_touch_motion.py
Remove-Item Env:VOLLEY_RUST_MOTION
Remove-Item Env:VOLLEY_RUST_REQUIRED
```

To use this kernel in a local CPU replay, opt in with `VOLLEY_RUST_MOTION=1`.
**Default remains Python.** Disabling the flag restores the original motion path.
Do not enable globally until annotated-match quality and 64-bit runtime compatibility have been verified.
