# API Reference

The full Rust API reference is auto-generated from source-code doc comments by `rustdoc` and published alongside this book. On the published documentation site, navigate directly to the links below.

## Browse Online

> **[Open the full API reference →](api/red_couch/index.html)**

### Key Entry Points

| Module | Description | Link |
|---|---|---|
| `red_couch` | Crate root — module registration, TCP listener, connection handling | [red_couch](api/red_couch/index.html) |
| `red_couch::protocol` | Binary protocol types, opcode enum, request parser, response encoder | [protocol](api/red_couch/protocol/index.html) |
| `red_couch::ascii` | ASCII text protocol parser and command dispatch | [ascii](api/red_couch/ascii/index.html) |
| `red_couch::meta` | Meta protocol parser, flag validation, command types | [meta](api/red_couch/meta/index.html) |

### Commonly Referenced Items

- [`Opcode`](api/red_couch/protocol/enum.Opcode.html) — all supported binary-protocol opcodes
- [`Request`](api/red_couch/protocol/struct.Request.html) — parsed binary request
- [`Header`](api/red_couch/protocol/struct.Header.html) — parsed request header
- [`ParseResult`](api/red_couch/protocol/enum.ParseResult.html) — parse outcome (Ok / Incomplete / error)
- [`try_parse_request`](api/red_couch/protocol/fn.try_parse_request.html) — primary request parser
- [`write_response`](api/red_couch/protocol/fn.write_response.html) — binary response encoder
- [`ResponseMeta`](api/red_couch/protocol/struct.ResponseMeta.html) — response metadata fields

## Generating Locally

```bash
# Build and open in your browser
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --open
```

The generated docs cover all public types, functions, and modules in the `red_couch` crate. They are rebuilt from source on every change, so they always reflect the current code.

## How It Works

The [GitHub Actions Pages workflow](https://github.com/fcenedes/RedCouch/blob/main/.github/workflows/pages.yml) builds both the mdBook site and the rustdoc output in a single job:

1. `mdbook build` produces the narrative documentation in `book/output/`.
2. `cargo doc --no-deps` produces the API reference in `target/doc/`.
3. The workflow copies `target/doc/` into `book/output/api/`, so the final Pages artifact has the structure:
   ```
   book/output/
   ├── index.html          ← mdBook site root
   ├── api/
   │   └── red_couch/      ← rustdoc API reference
   │       ├── index.html
   │       ├── protocol/
   │       ├── ascii/
   │       └── meta/
   └── ...                 ← other book chapters
   ```

The links on this page point into the `api/` subtree, so they work on the published site.

## CI Validation

The CI pipeline validates that both the API documentation and the book build cleanly on every push to `main` and on pull requests:

```bash
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps
mdbook build
```

This ensures documentation stays in sync with the code and catches broken links or build errors before merging.
