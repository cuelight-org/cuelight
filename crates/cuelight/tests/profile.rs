//! A frame resolved with its costs, and the text rasterizer's count.

use cuelight::{BitmapFont, Engine};

const FNT: &str = r#"info face="Blocks" size=3
common lineHeight=4 base=3 scaleW=2 scaleH=3 pages=1
page id=0 file="blocks_0.png"
char id=48 x=0 y=0 width=2 height=3 xoffset=0 yoffset=0 xadvance=3 page=0
char id=49 x=0 y=0 width=2 height=3 xoffset=0 yoffset=0 xadvance=3 page=0
"#;

const SHOW: &str = r##"{ "name": "p", "size": [32, 8],
  "variables": { "score": 0 }, "fonts": { "f": { "file": "blocks" } },
  "layers": [
    { "name": "box", "type": "group", "children": [
      { "name": "floor", "type": "shape", "shape": { "rect": [0, 0, 32, 8] }, "fill": "#202020" },
      { "name": "score", "type": "text", "font": "f", "text": "0",
        "bindings": [{ "property": "text", "variable": "score" }],
        "timelines": [{ "name": "in", "autoplay": true, "tracks": [
          { "property": "x", "keys": [{ "t": 0, "v": 0 }, { "t": 10, "v": 4 }] }] }] } ] },
    { "name": "hidden", "type": "shape", "shape": { "rect": [0, 0, 1, 1] }, "fill": "#FFFFFF", "visible": false } ] }"##;

fn engine() -> Engine {
    let mut engine = Engine::new();
    let font = BitmapFont::parse(FNT).unwrap();
    engine
        .set_font("blocks", font, vec![(2, 3, vec![255; 24])])
        .unwrap();
    engine.load_show(SHOW).unwrap();
    engine
}

#[test]
fn a_profile_is_the_frame_with_what_each_layer_cost() {
    let engine = engine();
    let profile = engine.profile().unwrap();
    // The very draw list, so the frame drawn is the frame measured.
    assert_eq!(profile.items, engine.resolved_layers().unwrap());
    // Every visible layer, in resolve order, the hidden one not.
    let names: Vec<&str> = profile.layers.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["floor", "score", "box"]);
    let kinds: Vec<&str> = profile.layers.iter().map(|c| c.kind).collect();
    assert_eq!(kinds, ["shape", "text", "group"]);
    // A group's total covers its children; its own share is what is left.
    let group = &profile.layers[2];
    assert!(group.total >= profile.layers[0].total + profile.layers[1].total);
    assert!(group.own <= group.total);
    assert_eq!(group.items, 2);
    assert_eq!(profile.layers[0].path_elements, 0);
    assert!(profile.layers[1].pixels > 0.0, "a raster of the text");
    assert_eq!(profile.bindings, 1);
    assert_eq!(profile.timelines, 1);
    assert!(profile.resolve >= group.total);
}

#[test]
fn the_text_rasterizer_counts_what_it_makes_afresh() {
    let mut engine = engine();
    let profile = engine.profile().unwrap();
    // The first frame rasterizes the string; the layer says so.
    assert_eq!(profile.layers[1].text_misses, 1);
    let stats = engine.text_stats();
    assert_eq!((stats.hits, stats.misses), (0, 1));
    assert!(stats.rasterized_bytes > 0 && stats.cached_bytes > 0);
    assert!(stats.cached_bytes <= stats.budget_bytes);
    // The same string again is a hit; a new one is another miss.
    let again = engine.profile().unwrap();
    assert_eq!(again.layers[1].text_misses, 0);
    engine.set_variable("score", 1.0);
    engine.profile().unwrap();
    let stats = engine.text_stats();
    assert_eq!((stats.hits, stats.misses), (1, 2));
    // Registering a font empties the cache and its count.
    engine
        .set_font(
            "other",
            BitmapFont::parse(FNT).unwrap(),
            vec![(2, 3, vec![255; 24])],
        )
        .unwrap();
    assert_eq!(engine.text_stats().misses, 0);
}
