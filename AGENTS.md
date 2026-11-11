# FramETape

MIT recording libraries and local tools. Preserve original channel payloads and explicit
frame mappings. A missing/dropped/repeated image must never masquerade as a captured frame.
No recordings, private project code, credentials or machine-specific paths in public commits.
Run `processqueue cpu cargo test --workspace` and review the diff before committing.
Use independent capture/channel producers; core bundle/annotation crate has no engine dependency.
Scratch: `.agents/scratch/` (excluded via `.git/info/exclude`). Builds: `target/` (same).

