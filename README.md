# mu-console

<p align="center">
  <a href="https://hook.lma1980.duckdns.org/jobs"><img src="https://hook.lma1980.duckdns.org/badges/LMA1980/mu-console/status.svg" alt="CI Status"/></a>
  <a href="https://hook.lma1980.duckdns.org/jobs"><img src="https://hook.lma1980.duckdns.org/badges/LMA1980/mu-console/score.svg" alt="Quality Score"/></a>
  <a href="https://hook.lma1980.duckdns.org/jobs"><img src="https://hook.lma1980.duckdns.org/badges/LMA1980/mu-console/coverage.svg" alt="Coverage"/></a>
  <a href="https://hook.lma1980.duckdns.org/jobs"><img src="https://hook.lma1980.duckdns.org/badges/LMA1980/mu-console/benchmark.svg" alt="Benchmark"/></a>
</p>

______________________________________________________________________

Unified CI/CD, Observability, Model-Runner, and Platform operations console for **ActionRunner**, **MuConDe**, **MuRegist**, **MuObserve**, and **Jero**. `mu-console` projects a skin-agnostic `ConsoleScene` across two skins—**`HtmlCoreSkin`** (`Html`, default) and **`WireframeSkin`** (optional CRT wireframe on active sessions)—driven by two WebAssembly modules: a UI module on the main thread and a data module in a dedicated Web Worker. It is hosted on `https://console.lma1980.duckdns.org` in parallel with `ActionRunner-web` on `https://hook.lma1980.duckdns.org` (where the GitHub `POST /webhook` endpoint permanently remains) so both clients operate side-by-side without loss of functionality.

See the [design specification](docs/design/README.md).

______________________________________________________________________

## Workspace Submodules

| Submodule                              | Target             | Licence                                       | Responsibility                                                                                        |
| -------------------------------------- | ------------------ | --------------------------------------------- | ----------------------------------------------------------------------------------------------------- |
| [`mu-console-protocol`](code/protocol) | native + wasm      | LGPL-3.0-only WITH LGPL-3.0-linking-exception | `UiCommand` / `DataEvent`, 8-area view models, versioned postcard codec                               |
| [`mu-console-theme`](code/theme)       | native + wasm      | LGPL-3.0-only WITH LGPL-3.0-linking-exception | Skin-agnostic `ConsoleScene`/`DialogModel`, `HtmlCoreSkin`, `WireframeSkin`, CSS tokens, font metrics |
| [`mu-console-layout`](code/layout)     | native + wasm      | LGPL-3.0-only WITH LGPL-3.0-linking-exception | Breakpoints, frame solver, metric-aware CRT column grid (zero page scroll)                            |
| [`mu-console-ui`](code/ui)             | wasm (main thread) | AGPL-3.0-only                                 | 8-area fail-closed frontend, passkey/elevation modals, two-skin projection, WASM bootstrap            |
| [`mu-console-data`](code/data)         | wasm (worker)      | AGPL-3.0-only                                 | Fetch, SSE client, normalized store, folio gating, lease scrubbing, \<=10 Hz coalescing               |

## Building & Verification

```bash
# Pre-PR gate sequence
(cd code && cargo fmt --all -- --check)
./scripts/format_docs.sh --check
cargo agents plugin validate Symposium.toml --no-check-crates
(cd code && cargo clippy --workspace --all-targets -- -D warnings)
(cd code && cargo test --workspace)
(cd code && cargo build --target wasm32-unknown-unknown -p mu-console-ui -p mu-console-data -p mu-console-protocol -p mu-console-theme)
./scripts/build-dist.sh
./scripts/check-dist-freshness.sh
```

### Fonts

Font binaries are **never committed**. `scripts/fetch-fonts.sh` downloads the pinned Nerd Fonts release zips (Hermit, OpenDyslexic), verifies their SHA-256, subsets them with `pyftsubset` (fontTools in a local `.venv`) using the code point list generated from `mu-console-theme`, checks that every required glyph survived, and writes WOFF2 files, each family's licence and a hash manifest into `dist/fonts/`.

```bash
./scripts/fetch-fonts.sh
(cd code && cargo run -q -p mu-console-theme --bin mu-console-subset-unicodes -- crt)   # pyftsubset --unicodes list
```

| Subset                                | Licence (as shipped in the Nerd Fonts zip)              |
| ------------------------------------- | ------------------------------------------------------- |
| Hurmit Nerd Font Mono (Regular, Bold) | SIL OFL 1.1 (Reserved Font Name "Hermit")               |
| OpenDyslexicM Nerd Font Mono          | Bitstream Vera–style licence (`LICENSE.txt` in the zip) |

## License

`mu-console` (the parent workspace and application) is licensed under the GNU Affero General Public License v3.0 (**AGPL-3.0-only**). See [LICENSE](LICENSE).

In accordance with the K3 platform Two-Tier Licensing Policy:

- Library crates ([`mu-console-protocol`](code/protocol), [`mu-console-theme`](code/theme), and [`mu-console-layout`](code/layout)) are licensed under the GNU Lesser General Public License v3.0 with the LGPL-3.0 Linking Exception (**LGPL-3.0-only WITH LGPL-3.0-linking-exception**).
- Application and service components ([`mu-console-ui`](code/ui), [`mu-console-data`](code/data)) are licensed under **AGPL-3.0-only**.
