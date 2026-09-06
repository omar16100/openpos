# Browser smoke tests

Not a product surface. Two pages that prove the core does in a browser what it
does natively, run by hand against a build of `openpos-bindings`.

```
cd bindings && wasm-pack build --target web --release --out-dir ../target/pkg
cd .. && cp -r target/pkg demo/pkg && python3 -m http.server 8080 --directory demo
```

- `index.html` rings a sale and checks the arithmetic against the figures the
  native suite asserts. The two must agree or the shared core is not shared.
- `opfs.html` rings a sale in a worker, destroys that worker, and opens a new
  till on the same OPFS files. It is the cold-start-offline claim, tested.

`pkg/` is a build artifact and is not committed.
