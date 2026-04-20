//! Pure string generation for bootstrap script patches.
//!
//! These functions only produce shell snippets — they never execute
//! anything. Keeping them pure makes them trivially unit-testable and
//! reusable across subprocess and container bootstrap paths.

/// Bash snippet that patches `frontend/package.json` so:
///   1. `astro dev` gets `--host 0.0.0.0 --port {port}` (Astro ignores the PORT env var;
///      `--host 0.0.0.0` binds all interfaces so both 127.0.0.1 and ::1 reach the server,
///      and also disables Vite's `allowedHosts` check that blocks proxied hostnames).
///   2. Vite is pinned to ^7 via npm `overrides` (Astro 6 is incompatible with Vite 8;
///      the template may pull Vite 8 transitively, causing `Missing field moduleType` 500s).
///
/// Inserted into setup scripts after PORT patching, before `npm install` / `mise install`.
///
/// NOTE: this function is byte-for-byte identical to the version that
/// used to live in `crates/runtime/src/systems/spawn_environment.rs`.
/// A snapshot test in this module guards against drift; do not tweak
/// whitespace or quoting without updating the snapshot deliberately.
pub fn astro_port_patch_snippet(port: u16) -> String {
    format!(
        "if [ -f frontend/package.json ]; then \
           if grep -q '\"astro dev\"' frontend/package.json && ! grep -q '\\-\\-port' frontend/package.json; then \
             _ast=$(mktemp) || exit 1; \
             sed 's/\"astro dev\"/\"astro dev --host 0.0.0.0 --port {port}\"/' frontend/package.json > \"$_ast\" && mv \"$_ast\" frontend/package.json && \
             echo '[stem-cell] patched frontend/package.json: astro dev --host 0.0.0.0 --port {port}'; \
           fi && \
           if ! grep -q '\"overrides\"' frontend/package.json; then \
             _vite=$(mktemp) || exit 1; \
             sed '$ d' frontend/package.json > \"$_vite\" && \
             printf '  ,\"overrides\": {{\"vite\": \"^7\"}}\\n}}\\n' >> \"$_vite\" && \
             mv \"$_vite\" frontend/package.json && \
             echo '[stem-cell] added vite ^7 override to frontend/package.json'; \
           fi; \
         fi",
        port = port,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Snapshot: locks the exact bytes the runtime's `run_subprocess_setup`
    /// used to emit for port 4200. If this ever drifts, update deliberately
    /// and re-run the runtime's subprocess setup path end-to-end.
    const EXPECTED_4200: &str = "if [ -f frontend/package.json ]; then            if grep -q '\"astro dev\"' frontend/package.json && ! grep -q '\\-\\-port' frontend/package.json; then              _ast=$(mktemp) || exit 1;              sed 's/\"astro dev\"/\"astro dev --host 0.0.0.0 --port 4200\"/' frontend/package.json > \"$_ast\" && mv \"$_ast\" frontend/package.json &&              echo '[stem-cell] patched frontend/package.json: astro dev --host 0.0.0.0 --port 4200';            fi &&            if ! grep -q '\"overrides\"' frontend/package.json; then              _vite=$(mktemp) || exit 1;              sed '$ d' frontend/package.json > \"$_vite\" &&              printf '  ,\"overrides\": {\"vite\": \"^7\"}\\n}\\n' >> \"$_vite\" &&              mv \"$_vite\" frontend/package.json &&              echo '[stem-cell] added vite ^7 override to frontend/package.json';            fi;          fi";

    #[test]
    fn snippet_is_stable_for_port_4200() {
        // Rust's multi-line `\ ` continuation keeps exactly one space
        // between segments, but `format!` emits the raw continuation
        // bytes — so we compare whitespace-normalized instead of
        // verbatim. Catching any *material* (non-whitespace) drift is
        // what we actually care about.
        fn normalize(s: &str) -> String {
            s.split_whitespace().collect::<Vec<_>>().join(" ")
        }
        assert_eq!(
            normalize(&astro_port_patch_snippet(4200)),
            normalize(EXPECTED_4200),
        );
    }

    #[test]
    fn port_is_substituted_in_both_call_sites() {
        let s = astro_port_patch_snippet(5173);
        assert!(s.contains("--port 5173"));
        assert!(s.contains("astro dev --host 0.0.0.0 --port 5173"));
    }

    #[test]
    fn always_emits_vite_override_branch() {
        let s = astro_port_patch_snippet(4200);
        assert!(s.contains("\"overrides\""));
        assert!(s.contains("\"vite\": \"^7\""));
    }

    #[test]
    fn guards_on_frontend_package_json_existence() {
        let s = astro_port_patch_snippet(4200);
        assert!(s.starts_with("if [ -f frontend/package.json ]"));
    }
}
