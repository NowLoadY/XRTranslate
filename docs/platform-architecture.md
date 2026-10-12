# Platform architecture

The host adapts operating-system capabilities to shared domain contracts.
Pipelines and plugins consume those contracts without choosing platform-specific
processes, storage paths, or device APIs.

## Ownership

| Owner | Responsibility |
|---|---|
| `xrtranslate-engine::language` | Canonical language identity, selection intent, and capability-based routing |
| `xrtranslate-config` | Normalized provider settings and declarative runtime catalogue |
| `xrtranslate-assets` | Model catalogue, package installation, activation, and removal |
| `xrtranslate-download` | Download sources and shared transfer mechanics |
| Backend runtime plan | Resolve compatible resources and construct inference adapters |
| `xrtranslate-supervisor` | Managed process lifecycle from a resolved launch specification |
| Host platform adapters | Native capture, devices, windows, input, media sessions, and system integration |

UI and pipelines use shared capability metadata. Provider and model names,
archive filenames, or operating-system names must not become parallel selection
rules in individual consumers. Protocol differences stay inside connectors;
platform differences stay inside host adapters.

## Configuration and resources

Model packages and executable runtimes are separate resources. Catalogues
express capabilities, language support, hardware requirements, and compatible
runtime assets. Installation, onboarding, and backend startup consume the same
resolved plan. The selected execution class must not silently change when a
required accelerator or runtime is unavailable.

The resource owner controls installation, staging, readiness, and deletion.
Installers reuse the shared downloader; UI submits resource identities rather
than manipulating installation paths. Runtime activation publishes a complete,
compatible resource set before consumers start using it. External resources
remain distinct from application-managed resources.

`RuntimeLayout` owns writable application locations. User configuration,
credentials, histories, and caches remain separate from immutable release
resources. Releases ship public defaults and curated assets; local runtime data
and secrets stay outside release staging and survive application replacement.

## Host capabilities

Native adapters expose focused capabilities with explicit availability and
errors. Unsupported functionality must not be represented by fake devices or a
second feature-specific pipeline. Platform discovery and native handles belong
to the host; saved domain state uses stable identities rather than live handles.

Audio Studio owns the selected audio graph and its routing intent. The host
executes neutral routes and owns device capture and output lifecycles. UI,
validation, and execution read the same routing state. Audio routing and
recognition sessions have separate lifecycles while sharing the selected input.
Optional system integrations retain their own resource ownership and cleanup.

Language intent is captured with each task and validated through shared language
capabilities. UI, backend, and adapters use canonical labels; a provider's wire
aliases stay inside its adapter. Model support and language identity remain
separate concepts.

## Platform and release extension

Add target metadata for existing runtime families and implement only the native
capabilities that differ. Reuse shared download, installation, inference, and
session contracts. Platform support must not require a second domain pipeline.

Application versioning and update discovery are shared. Platform installers own
package validation and system installation, while build and signing procedures
remain in their tooling. Release packaging preserves licensing and attribution
alongside the assets it distributes.

See [provider integration](providers/README.md) for connector work and
[the refactoring contract](refactoring-contract.md) for dependency rules.
