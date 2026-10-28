//! Tile 缓存模块（LRU，同步渲染）
//!
//! 预取线程已移除——LOK 不是线程安全的，Document 是 !Send。
//! 后续确认 LOK 线程模型后再考虑重新引入异步预取。

use lru::LruCache;
use std::num::NonZeroUsize;

pub const TILE_PIXELS: i32 = 512;
pub const DPI: f32 = 192.0;
pub const MAX_TILES: usize = 64;

// ---------------------------------------------------------------------------
// 坐标工具
// ---------------------------------------------------------------------------

pub fn twips_to_pixels(twips: i32, dpi: f32) -> f32 {
    twips as f32 / 1440.0 * dpi
}

pub fn pixels_to_twips(pixels: f32, dpi: f32) -> i32 {
    (pixels / dpi * 1440.0).round() as i32
}

pub struct TileCoord {
    pub col: i32,
    pub row: i32,
}

impl TileCoord {
    pub fn new(col: i32, row: i32) -> Self {
        Self { col, row }
    }

    pub fn to_twips(&self, dpi: f32) -> (i32, i32, i32, i32) {
        let tile_size_twips = pixels_to_twips(TILE_PIXELS as f32, dpi);
        let x = self.col * tile_size_twips;
        let y = self.row * tile_size_twips;
        (x, y, tile_size_twips, tile_size_twips)
    }
}

pub fn prefetch_coords(
    visible_cols: std::ops::RangeInclusive<i32>,
    visible_rows: std::ops::RangeInclusive<i32>,
) -> Vec<(i32, i32)> {
    let col_start = *visible_cols.start();
    let col_end = *visible_cols.end();
    let row_start = *visible_rows.start();
    let row_end = *visible_rows.end();

    let mut coords = Vec::new();
    for c in (col_start - 1)..=(col_end + 1) {
        coords.push((c, row_start - 1));
        coords.push((c, row_end + 1));
    }
    for r in row_start..=row_end {
        coords.push((col_start - 1, r));
        coords.push((col_end + 1, r));
    }
    coords.retain(|&(c, r)| c >= 0 && r >= 0);
    coords
}

// ---------------------------------------------------------------------------
// PaintTile trait
// ---------------------------------------------------------------------------

pub trait PaintTile {
    fn paint_tile(
        &self,
        buffer: &mut Vec<u8>,
        canvas_width: i32,
        canvas_height: i32,
        tile_x: i32,
        tile_y: i32,
        tile_width: i32,
        tile_height: i32,
    );
}

// ---------------------------------------------------------------------------
// TileCache（纯同步，无后台线程）
// ---------------------------------------------------------------------------

type TileKey = (i32, i32);
type TileData = Vec<u8>;

pub struct TileCache {
    tiles: LruCache<TileKey, TileData>,
    dpi: f32,
}

impl TileCache {
    pub fn new(dpi: f32) -> Self {
        Self {
            tiles: LruCache::new(NonZeroUsize::new(MAX_TILES).unwrap()),
            dpi,
        }
    }

    /// 缓存命中直接返回，未命中则同步渲染后缓存
    pub fn get_or_render(
        &mut self,
        doc: &dyn PaintTile,
        col: i32,
        row: i32,
    ) -> &[u8] {
        if !self.tiles.contains(&(col, row)) {
            let coord = TileCoord::new(col, row);
            let (tx, ty, tw, th) = coord.to_twips(self.dpi);
            let mut buf = Vec::new();
            doc.paint_tile(&mut buf, TILE_PIXELS, TILE_PIXELS, tx, ty, tw, th);
            self.tiles.put((col, row), buf);
        }
        self.tiles.get(&(col, row)).unwrap()
    }

    /// 同步预取：直接在当前线程渲染外围一圈（不在缓存里的）
    pub fn prefetch(&mut self, doc: &dyn PaintTile,
        visible_cols: std::ops::RangeInclusive<i32>,
        visible_rows: std::ops::RangeInclusive<i32>,
    ) {
        let coords = prefetch_coords(visible_cols, visible_rows);
        for (col, row) in coords {
            if !self.tiles.contains(&(col, row)) {
                self.get_or_render(doc, col, row);
            }
        }
    }

    pub fn evict(&mut self, col: i32, row: i32) {
        self.tiles.pop(&(col, row));
    }

    pub fn len(&self) -> usize {
        self.tiles.len()
    }

    pub fn invalidate_all(&mut self) {
        self.tiles.clear();
    }

    pub fn set_dpi(&mut self, dpi: f32) {
        self.dpi = dpi;
    }

    pub fn is_empty(&self) -> bool {
        self.tiles.is_empty()
    }
}

// ---------------------------------------------------------------------------
// 单元测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    struct MockDoc;
    impl PaintTile for MockDoc {
        fn paint_tile(&self, buffer: &mut Vec<u8>, w: i32, h: i32,
            _: i32, _: i32, _: i32, _: i32) {
            buffer.resize((w * h * 4) as usize, 0);
        }
    }

    #[test]
    fn twips_roundtrip() {
        let px = 256.0_f32;
        let twips = pixels_to_twips(px, DPI);
        let back = twips_to_pixels(twips, DPI);
        assert!((back - px).abs() < 1.0);
    }

    #[test]
    fn tile_coord_origin() {
        let coord = TileCoord::new(0, 0);
        let (x, y, w, h) = coord.to_twips(DPI);
        assert_eq!(x, 0);
        assert_eq!(y, 0);
        assert!(w > 0);
        assert_eq!(w, h);
    }

    #[test]
    fn tile_coord_next() {
        let c0 = TileCoord::new(0, 0);
        let c1 = TileCoord::new(1, 0);
        let (x0, _, w, _) = c0.to_twips(DPI);
        let (x1, _, _, _) = c1.to_twips(DPI);
        assert_eq!(x1, x0 + w);
    }

    #[test]
    fn prefetch_coords_basic() {
        let coords = prefetch_coords(0..=1, 0..=1);
        assert!(coords.contains(&(2, 0)));
        assert!(coords.contains(&(2, 1)));
        assert!(coords.contains(&(0, 2)));
        assert!(coords.contains(&(1, 2)));
        assert!(!coords.iter().any(|&(c, r)| c < 0 || r < 0));
    }

    #[test]
    fn lru_eviction_with_mock() {
        let doc = MockDoc;
        let mut cache = TileCache::new(DPI);
        for i in 0..=(MAX_TILES as i32) {
            cache.get_or_render(&doc, i, 0);
        }
        assert!(cache.len() <= MAX_TILES);
    }

    #[test]
    fn prefetch_fills_cache() {
        let doc = MockDoc;
        let mut cache = TileCache::new(DPI);
        cache.prefetch(&doc, 0..=1, 0..=1);
        // 外围一圈应该有数据进缓存
        assert!(cache.len() > 0);
    }
}
