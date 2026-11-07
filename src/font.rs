//! The one font strobe draws with (Inter, embedded): result cards and badges, terminal snapshots. Images
//! come out the same on every machine, with no system fonts.

use resvg::usvg;
use std::sync::{Arc, OnceLock};

pub const REGULAR: &[u8] = include_bytes!("../assets/fonts/Inter-Regular.ttf");
pub const SEMIBOLD: &[u8] = include_bytes!("../assets/fonts/Inter-SemiBold.ttf");

/// The embedded faces, loaded once; `Inter` is also the sans-serif family.
pub fn database() -> Arc<usvg::fontdb::Database> {
    static DB: OnceLock<Arc<usvg::fontdb::Database>> = OnceLock::new();
    DB.get_or_init(|| {
        let mut db = usvg::fontdb::Database::new();
        db.load_font_data(REGULAR.to_vec());
        db.load_font_data(SEMIBOLD.to_vec());
        db.set_sans_serif_family("Inter");
        Arc::new(db)
    })
    .clone()
}

/// SVG options that draw text in the embedded font.
pub fn options() -> usvg::Options<'static> {
    let mut o = usvg::Options { font_family: "Inter".into(), ..Default::default() };
    o.fontdb = database();
    o
}
