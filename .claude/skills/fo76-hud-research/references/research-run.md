# Research-run contract

Keep this compact record outside the repository alongside the downloaded corpus:

```json
{
  "schema_version": 1,
  "run_id": "timestamp-and-random-suffix",
  "scope": {"game": "fallout76", "mod_ids": [], "surfaces": []},
  "inputs": [{"ref": "local-or-public-ref", "sha256": "...", "local_only": true}],
  "toolchain": [{"name": "ba2|ffdec|other-static-tool", "version": "...", "sha256": "..."}],
  "stages": {"metadata": "complete", "archives": "complete", "swf": "complete", "permissions": "needs-review"},
  "evidence": [],
  "unknowns": [],
  "payloads": {"raw_kept_external": true, "tracked_in_repository": false}
}
```

Artifacts should contain hashes, stable IDs, bounded names, and evidence references—not raw
decompiled source, game-owned bytes, signed URLs, cookies, or API keys. Keep output deterministic
and stable-sort lists. When comparing two packages, join on domain/mod/file ID plus source hash
and logical entry path.

The minimum useful offline pass is an archive inventory, a bounded SWF structural report, explicit
member selections, and a SHA-256 manifest. A static parse can report format facts; it cannot
certify runtime load, security, or author permission.
