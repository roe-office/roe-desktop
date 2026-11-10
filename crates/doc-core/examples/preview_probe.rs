use doc_core::*;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = args.get(1).expect("usage: preview_probe <file.docx> [out.png]");
    let out = args.get(2).cloned().unwrap_or("/tmp/preview_probe.png".to_string());
    let bytes = std::fs::read(path).expect("read file");
    let mut s = new_tile_cache();
    let h = s.open_document_from_bytes(&bytes, 99);
    println!("handle={} type={}", h, s.get_doc_type());
    println!("page_count={}", s.get_page_count());
    let size = s.get_writer_document_size();
    println!("doc_size={:?}", size);
    let tops = s.get_writer_page_tops();
    println!("page_tops(first5)={:?}", tops.iter().take(5).collect::<Vec<_>>());
    let (doc_w, _ch, _pc) = size.expect("no writer size");
    let vh: f32 = 900.0;
    let scale = doc_bitmap_scale();
    println!("bitmap_scale={}", scale);
    let rgba = s.render_writer_viewport(0.0, doc_w, vh);
    let pw = (doc_w * scale).round() as u32;
    let ph = (vh * scale).round() as u32;
    println!("rgba.len={} expect={}", rgba.len(), (pw * ph * 4) as usize);
    if rgba.is_empty() { println!("render failed"); return; }
    let img = image::RgbaImage::from_raw(pw, ph, rgba).expect("bad buffer size");
    img.save(&out).expect("save png");
    println!("saved {}x{} -> {}", pw, ph, out);
}
