#![allow(dead_code)]
// 通用选择框几何：纯函数，不依赖 Slint。矩形 (x,y,w,h) 为未旋转时的左上角和尺寸，
// rot 为绕矩形中心的旋转角（度，顺时针，屏幕坐标 y 向下）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct R { pub x: f32, pub y: f32, pub w: f32, pub h: f32, pub rot: f32 }

pub fn rot_vec(x: f32, y: f32, deg: f32) -> (f32, f32) {
    let (s, c) = deg.to_radians().sin_cos();
    (x * c - y * s, x * s + y * c)
}

pub fn center(r: R) -> (f32, f32) { (r.x + r.w / 2.0, r.y + r.h / 2.0) }

pub fn point_in(r: R, px: f32, py: f32, slack: f32) -> bool {
    let (cx, cy) = center(r);
    let (lx, ly) = rot_vec(px - cx, py - cy, -r.rot);
    lx.abs() <= r.w / 2.0 + slack && ly.abs() <= r.h / 2.0 + slack
}

pub fn compute_move(start: R, dx: f32, dy: f32) -> R {
    R { x: start.x + dx, y: start.y + dy, ..start }
}

pub fn compute_rotate(start: R, m0: (f32, f32), m: (f32, f32), snap15: bool) -> R {
    let (cx, cy) = center(start);
    let a0 = (m0.1 - cy).atan2(m0.0 - cx);
    let a1 = (m.1 - cy).atan2(m.0 - cx);
    let mut deg = start.rot + (a1 - a0).to_degrees();
    if snap15 { deg = (deg / 15.0).round() * 15.0; }
    while deg > 180.0 { deg -= 360.0; }
    while deg <= -180.0 { deg += 360.0; }
    R { rot: deg, ..start }
}

/// handle: n/s/e/w/ne/nw/se/sw。shift 只对四角生效(等比)；center_mode 以中心为锚点。
/// 缩到 min 为止，不翻转。锚点（对边/对角，或中心）在世界坐标里保持不动。
pub fn compute_resize(start: R, handle: &str, m0: (f32, f32), m: (f32, f32),
                      shift: bool, center_mode: bool, min: f32) -> Option<R> {
    let ex: f32 = if handle.contains('e') { 1.0 } else if handle.contains('w') { -1.0 } else { 0.0 };
    let ey: f32 = if handle.contains('s') { 1.0 } else if handle.contains('n') { -1.0 } else { 0.0 };
    if ex == 0.0 && ey == 0.0 { return None; }

    let (dlx, dly) = rot_vec(m.0 - m0.0, m.1 - m0.1, -start.rot);
    let hw = start.w / 2.0;
    let hh = start.h / 2.0;
    let (mut l, mut r, mut t, mut b) = (-hw, hw, -hh, hh);
    if ex > 0.0 { r += dlx; if center_mode { l -= dlx; } }
    if ex < 0.0 { l += dlx; if center_mode { r -= dlx; } }
    if ey > 0.0 { b += dly; if center_mode { t -= dly; } }
    if ey < 0.0 { t += dly; if center_mode { b -= dly; } }

    let mut nw = r - l;
    let mut nh = b - t;
    if shift && ex != 0.0 && ey != 0.0 {
        let k = (nw / start.w.max(1.0)).max(nh / start.h.max(1.0))
            .max(min / start.w.max(1.0)).max(min / start.h.max(1.0));
        nw = start.w * k;
        nh = start.h * k;
    }
    nw = nw.max(min);
    nh = nh.max(min);

    let (l2, r2) = if center_mode || ex == 0.0 { (-nw / 2.0, nw / 2.0) }
        else if ex > 0.0 { (-hw, -hw + nw) } else { (hw - nw, hw) };
    let (t2, b2) = if center_mode || ey == 0.0 { (-nh / 2.0, nh / 2.0) }
        else if ey > 0.0 { (-hh, -hh + nh) } else { (hh - nh, hh) };

    let (wx, wy) = rot_vec((l2 + r2) / 2.0, (t2 + b2) / 2.0, start.rot);
    let (scx, scy) = center(start);
    let (ncx, ncy) = (scx + wx, scy + wy);
    Some(R { x: ncx - nw / 2.0, y: ncy - nh / 2.0, w: nw, h: nh, rot: start.rot })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn near(a: f32, b: f32) -> bool { (a - b).abs() < 0.01 }
    fn corner(r: R, sx: f32, sy: f32) -> (f32, f32) {
        let (cx, cy) = center(r);
        let (wx, wy) = rot_vec(sx * r.w / 2.0, sy * r.h / 2.0, r.rot);
        (cx + wx, cy + wy)
    }

    #[test]
    fn resize_se_unrotated() {
        let s = R { x: 100.0, y: 100.0, w: 200.0, h: 100.0, rot: 0.0 };
        let n = compute_resize(s, "se", (300.0, 200.0), (350.0, 230.0), false, false, 8.0).unwrap();
        assert!(near(n.x, 100.0) && near(n.y, 100.0) && near(n.w, 250.0) && near(n.h, 130.0), "{:?}", n);
    }

    #[test]
    fn corner_anchor_fixed_when_rotated() {
        let s = R { x: 100.0, y: 100.0, w: 200.0, h: 120.0, rot: 30.0 };
        let cases = [("se", -1.0, -1.0), ("nw", 1.0, 1.0), ("ne", -1.0, 1.0), ("sw", 1.0, -1.0)];
        let deltas = [(30.0, 50.0), (-80.0, 20.0), (-500.0, -500.0), (120.0, -40.0)];
        for (h, ax, ay) in cases {
            let before = corner(s, ax, ay);
            for (dx, dy) in deltas {
                let n = compute_resize(s, h, (400.0, 300.0), (400.0 + dx, 300.0 + dy), false, false, 8.0).unwrap();
                let after = corner(n, ax, ay);
                assert!(near(before.0, after.0) && near(before.1, after.1),
                        "handle {h} delta ({dx},{dy}) anchor moved {:?} -> {:?}", before, after);
            }
        }
    }

    #[test]
    fn center_mode_keeps_center() {
        let s = R { x: 100.0, y: 100.0, w: 200.0, h: 100.0, rot: 0.0 };
        let n = compute_resize(s, "se", (300.0, 200.0), (340.0, 220.0), false, true, 8.0).unwrap();
        let (a, b) = center(s);
        let (c, d) = center(n);
        assert!(near(a, c) && near(b, d) && near(n.w, 280.0) && near(n.h, 140.0), "{:?}", n);
    }

    #[test]
    fn shift_keeps_aspect() {
        let s = R { x: 100.0, y: 100.0, w: 200.0, h: 100.0, rot: 0.0 };
        let n = compute_resize(s, "se", (300.0, 200.0), (400.0, 200.0), true, false, 8.0).unwrap();
        assert!(near(n.w, 300.0) && near(n.h, 150.0) && near(n.x, 100.0) && near(n.y, 100.0), "{:?}", n);
    }

    #[test]
    fn min_size_clamps_without_flip() {
        let s = R { x: 100.0, y: 100.0, w: 200.0, h: 100.0, rot: 0.0 };
        let n = compute_resize(s, "se", (300.0, 200.0), (0.0, 0.0), false, false, 8.0).unwrap();
        assert!(near(n.w, 8.0) && near(n.h, 8.0) && near(n.x, 100.0) && near(n.y, 100.0), "{:?}", n);
    }

    #[test]
    fn rotate_quarter_turn() {
        let s = R { x: 100.0, y: 100.0, w: 200.0, h: 100.0, rot: 0.0 };
        let n = compute_rotate(s, (300.0, 150.0), (200.0, 250.0), false);
        assert!(near(n.rot, 90.0), "{:?}", n);
    }
}


/// 直线端点：由矩形、旋转、翻转得出 (起点x, 起点y, 终点x, 终点y)。
/// 未翻转时起点为矩形左上、终点为右下；先翻转后旋转（与后端渲染一致）。
pub fn line_endpoints(x: f32, y: f32, w: f32, h: f32, rot: f32, fh: bool, fv: bool) -> (f32, f32, f32, f32) {
    let (cx, cy) = (x + w / 2.0, y + h / 2.0);
    let sx = if fh { -1.0 } else { 1.0 };
    let sy = if fv { -1.0 } else { 1.0 };
    let (s, c) = rot.to_radians().sin_cos();
    let pt = |lx: f32, ly: f32| {
        let (lx, ly) = (lx * sx, ly * sy);
        (cx + lx * c - ly * s, cy + lx * s + ly * c)
    };
    let a = pt(-w / 2.0, -h / 2.0);
    let b = pt(w / 2.0, h / 2.0);
    (a.0, a.1, b.0, b.1)
}
