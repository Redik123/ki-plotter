//! Geometry and cut-job pipeline for KI Plotter.
//!
//! Everything is in millimetres, in document space: `x` runs along the roll
//! (the feed direction), `y` runs across it and points down on screen. The
//! machine frame only appears inside [`job::build_job`].
//!
//! Entry points (one per Tauri command): [`build_job`], [`make_weeding`],
//! [`make_tiles`]. All request/response types serialise with the camelCase
//! field names of `src/model/types.ts`; errors are French strings meant to
//! be shown as is.

pub mod blade;
pub mod dmpl;
pub mod flatten;
pub mod hpgl;
pub mod job;
pub mod num;
pub mod order;
pub mod path;
pub mod rect;
pub mod tile;
pub mod transform;
pub mod weed;

pub use flatten::{Polyline, flatten_path, flatten_shape, polygons_of};
pub use job::{
    CutSettings, JobRequest, JobResult, JobStats, MachineSettings, Move, Protocol, build_job,
};
pub use path::{FillRule, IDENTITY, Mat, PathData, Seg, Shape};
pub use rect::Rect;
pub use tile::{Tile, TileRequest, make_tiles};
pub use transform::{path_bounds, shape_bounds, shapes_bounds, transform_path};
pub use weed::{WeedRequest, make_weeding};
