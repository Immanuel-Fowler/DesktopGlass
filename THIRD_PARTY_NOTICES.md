# Third-party software

DesktopGlass includes the following open-source software. Their licenses apply to those parts only.

## SwiftTerm

- https://github.com/migueldeicaza/SwiftTerm
- License: MIT — see the LICENSE file in the SwiftTerm repository (fetched by Swift Package Manager into
  `.build/checkouts/SwiftTerm/LICENSE`). Its copyright and permission notice must be kept when you
  redistribute DesktopGlass in binary form.

## swift-argument-parser

- https://github.com/apple/swift-argument-parser
- License: Apache 2.0. Resolved by Swift Package Manager as a dependency of SwiftTerm (listed in `Package.resolved`).

## Mermaid

- https://github.com/mermaid-js/mermaid
- License: MIT. `mermaid.min.js` is bundled into the app at build time (`scripts/build-app.sh`) to render diagrams
  in Markdown Preview; its license is copied to `Contents/Resources/mermaid-LICENSE.txt`.

