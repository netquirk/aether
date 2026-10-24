# Aether

A fixture README. Its "Using Aether as a library" example names a type that `aether-agent-core`
does not export, so compiling this file must fail and name that type.

## Using Aether as a library

```rust
use aether_core::core::NotExportedByAetherCore;

fn main() {
    let _ = NotExportedByAetherCore::new();
}
```
