//! Strict Asset & Profile Contract (TOML <-> Dual-View 48x48 px SVG) for `dendriForge` v2
//! (`03-asset-and-profile-contract.md` §1–§7, ADRs 013–016, 025, 029, 030).
//!
//! Enforces:
//! - Strict `#[serde(deny_unknown_fields)]` schemas for component profiles, board profiles,
//!   declarative composite sub-circuits, provenance blocks, and `review.toml` sign-offs.
//! - Canonical 48x48 px grid (`1 u = 48 px`, sub-grid `24 px` / `12 px`), 8x8 px `<g id="df-pins">`
//!   semantic pin markers (`dendri-theme::PinClass`), zero collisions, and exact 1:1 port parity
//!   across `profile.toml`, `physical.svg`, and `schematic.svg`.
//! - IEC 81346-2 / IEC 61082 reference-designation prefixes (`DEF-009`).
//! - Canonical SVG sanitization & single-`usvg` headless `resvg` + `tiny-skia` SHA-256 render hashing.
//! - Complete 13-rule `dendri asset-check` validator and explicit human `dendri asset-approve` helper.

pub mod ref_des;
pub mod schema;
pub mod svg;
pub mod transform;
pub mod validator;

pub use ref_des::{RefDesClass, format_ref_designator, is_valid_ref_designator_prefix};
pub use schema::{
    BindingKind, BoardChannelSpec, BoardCodecSpec, BoardElectricalSpec, BoardExecutionSpec,
    BoardMeta, BoardPort, BoardProfile, BoardPwmSpec, BoardSvgSpec, ComponentMeta, ComponentPort,
    ComponentProfile, ComponentSvgSpec, CompositePart, CompositeSubcircuit, CompositeWire, CpuSpec,
    DisplaySpec, ExecutionCostSpec, GeometrySpec, ParamKind, ParamSpec, PhysicsSpec,
    PortCapability, PortDomain, Provenance, ProvenanceSource, REGISTERED_PRIMITIVE_MODELS,
    ReviewChecklist, ReviewSignOff, SvgBinding, is_registered_primitive_model,
};
pub use svg::{
    CANONICAL_GRID_PX, CANONICAL_PIN_PX, ParsedPin, ParsedSvg, SvgError, SvgProvenanceMeta,
    canonicalize_and_sanitize_svg, compute_render_sha256, parse_and_validate_svg,
};
pub use transform::{Affine2D, Pose2D};
pub use validator::{
    AssetCatalog, AssetCheckError, AssetCheckReport, AssetViolation, approve_asset_dir,
    validate_assets_dir,
};
