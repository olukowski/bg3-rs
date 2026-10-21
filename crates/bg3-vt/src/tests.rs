//! Install-free tests on self-authored synthetic tile sets and streams.

use std::collections::HashMap;

use crate::fourcc::{self, FourCcNode, FourCcValue};
use crate::*;

// ---------------------------------------------------------------- FastLZ

#[test]
fn fastlz_literals_and_short_match_level1() {
    // "abc" literal run, then a match of len 6 at distance 3 → "abcabcabc".
    // ctrl for match: len field = 6-2 = 4 → (4 << 5) | high(0); low = distance-1 = 2.
    let stream = [2, b'a', b'b', b'c', 4 << 5, 2];
    assert_eq!(fastlz::decompress(&stream, 64).unwrap(), b"abcabcabc");
}

#[test]
fn fastlz_long_match_level1() {
    // One literal 'x', then a run copy of length 7+2+10 = 19 at distance 1.
    let stream = [0, b'x', 7 << 5, 10, 0];
    assert_eq!(fastlz::decompress(&stream, 64).unwrap(), vec![b'x'; 20]);
}

#[test]
fn fastlz_level2_extended_length_and_far_distance() {
    // Level 2 (first byte top bits = 001): literal "ab", then a len run of 9 + 255 + 3 = 267
    // at distance 2.
    let stream = [(1 << 5) | 1, b'a', b'b', 7 << 5, 255, 3, 1];
    let out = fastlz::decompress(&stream, 1024).unwrap();
    assert_eq!(out.len(), 2 + 9 + 255 + 3);
    assert!(
        out.chunks(2)
            .all(|c| c[0] == b'a' && c.get(1).is_none_or(|&b| b == b'b'))
    );

    // Far match: 8193 literal bytes, then copy 3 bytes from distance 8192 + 1 (escape form).
    let mut stream = vec![1 << 5]; // level 2, literal of 1
    stream.push(0);
    let mut expect = vec![0u8];
    while expect.len() < 8193 {
        let n = (8193 - expect.len()).min(32);
        stream.push((n - 1) as u8);
        for i in 0..n {
            let b = (expect.len() + i) as u8;
            stream.push(b);
        }
        let base = expect.len();
        expect.extend((0..n).map(|i| (base + i) as u8));
    }
    // match len 3 (field 1), high = 31, low = 255 → far; distance = 1 + 8191 + 1 = 8193 back.
    stream.extend_from_slice(&[(1 << 5) | 31, 255, 0, 1]);
    let out = fastlz::decompress(&stream, 1 << 16).unwrap();
    let start = expect.len() - 8193;
    let copied: Vec<u8> = expect[start..start + 3].to_vec();
    assert_eq!(&out[..expect.len()], &expect[..]);
    assert_eq!(&out[expect.len()..], &copied[..]);
}

#[test]
fn fastlz_rejects_bad_input() {
    assert_eq!(
        fastlz::decompress(&[0, 1, 2 << 5, 9], 64),
        Err(fastlz::FastLzError::BadDistance)
    );
    assert_eq!(
        fastlz::decompress(&[5, 1], 64),
        Err(fastlz::FastLzError::Truncated)
    );
    assert_eq!(
        fastlz::decompress(&[3 << 5], 64),
        Err(fastlz::FastLzError::Level(4))
    );
    assert!(matches!(
        fastlz::decompress(&[0, 1, 7 << 5, 200, 0], 64),
        Err(fastlz::FastLzError::Overflow(_))
    ));
}

/// All-literal FastLZ level-1 encoding (valid, if not small).
fn fastlz_store(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    for c in data.chunks(32) {
        out.push((c.len() - 1) as u8);
        out.extend_from_slice(c);
    }
    out
}

// ---------------------------------------------------------------- BC

#[test]
fn bc_solid_blocks_round_trip() {
    for f in [BcFormat::Bc1, BcFormat::Bc3, BcFormat::Bc4, BcFormat::Bc5] {
        let block = f.solid_block([255, 0, 255, 128]);
        assert_eq!(block.len(), f.block_bytes());
        let px = f.decode_block(&block);
        let want = match f {
            BcFormat::Bc1 => [255, 0, 255, 255],
            BcFormat::Bc3 => [255, 0, 255, 128],
            BcFormat::Bc4 => [255, 255, 255, 255],
            BcFormat::Bc5 => [255, 0, 0, 255],
        };
        assert!(px.iter().all(|p| *p == want), "{f:?}: {:?}", px[0]);
    }
}

#[test]
fn bc3_interpolation() {
    // alpha 0..=255 endpoints 255, 0 → 8-value palette; colour endpoints white/black.
    let mut b = vec![255, 0];
    // indices: pixel i gets alpha index i % 8.
    let mut bits = 0u64;
    for i in 0..16u64 {
        bits |= (i % 8) << (3 * i);
    }
    b.extend_from_slice(&bits.to_le_bytes()[..6]);
    b.extend_from_slice(&0xFFFFu16.to_le_bytes());
    b.extend_from_slice(&0u16.to_le_bytes());
    b.extend_from_slice(&0b11_10_01_00u32.to_le_bytes()); // pixels 0..3 → indices 0,1,2,3
    let px = BcFormat::Bc3.decode_block(&b);
    assert_eq!(px[0], [255, 255, 255, 255]);
    assert_eq!(px[1], [0, 0, 0, 0]);
    assert_eq!(px[2][3], (6 * 255 / 7) as u8);
    assert_eq!(px[2][..3], [170, 170, 170]);
    assert_eq!(px[3][..3], [85, 85, 85]);
}

// ---------------------------------------------------------------- synthetic tile set

const TILE: u32 = 16;
const BORDER: u32 = 4;
const PAGE: u32 = 4096;

/// A BC3 tile whose block (bx, by) is a solid colour derived from (tag, bx, by).
fn bc3_tile(tag: u8) -> Vec<u8> {
    let mut out = Vec::new();
    for by in 0..TILE / 4 {
        for bx in 0..TILE / 4 {
            out.extend(BcFormat::Bc3.solid_block(block_color(tag, bx, by)));
        }
    }
    out
}

fn block_color(tag: u8, bx: u32, by: u32) -> [u8; 4] {
    // Values that survive RGB565 exactly: multiples of 0x21-ish aren't needed if we only compare
    // through the same encoder, but keep them distinct.
    [tag, (bx * 64) as u8, (by * 64) as u8, 255]
}

/// Store the current length of `d` as a u64 at `at` (a forward offset).
fn here64(d: &mut [u8], at: usize) {
    let v = d.len() as u64;
    d[at..at + 8].copy_from_slice(&v.to_le_bytes());
}

struct Fixture {
    gts: Vec<u8>,
    pages: HashMap<String, Vec<u8>>,
}

/// Build a tile set: 2 layers, 2 levels (2×1 tiles, 1×1 tile), one texture "abc" covering the
/// whole 16×8 level-0 image. Layer 0: level 0 = [BC tile (raw), uniform tile], level 1 = BC tile
/// (FastLZ). Layer 1: only stored at level 1 (level-0 entries fall back to it).
fn fixture() -> Fixture {
    // ---- page file: one page, 4 chunks
    let mip = vec![0u8; BcFormat::Bc3.image_bytes(TILE / 2, TILE / 2)];
    let tile_a = [bc3_tile(10), mip.clone()].concat();
    let tile_c = [bc3_tile(30), mip.clone()].concat();
    let tile_d = [bc3_tile(40), mip].concat();
    let chunks: Vec<(u32, u32, Vec<u8>)> = vec![
        (9, 0, tile_a),                  // raw BC
        (0, 2, vec![200, 100, 50, 255]), // uniform
        (9, 1, fastlz_store(&tile_c)),   // FastLZ BC, layer 0 level 1
        (9, 1, fastlz_store(&tile_d)),   // FastLZ BC, layer 1 level 1
    ];
    let mut gtp = Vec::new();
    gtp.extend_from_slice(b"GRAP");
    gtp.extend_from_slice(&4u32.to_le_bytes());
    gtp.extend_from_slice(&[7; 16]);
    let table_at = gtp.len();
    gtp.extend_from_slice(&(chunks.len() as u32).to_le_bytes());
    gtp.resize(gtp.len() + 4 * chunks.len(), 0);
    for (i, (codec, pb, data)) in chunks.iter().enumerate() {
        let off = gtp.len() as u32;
        gtp[table_at + 4 + 4 * i..table_at + 8 + 4 * i].copy_from_slice(&off.to_le_bytes());
        gtp.extend_from_slice(&codec.to_le_bytes());
        gtp.extend_from_slice(&pb.to_le_bytes());
        gtp.extend_from_slice(&(data.len() as u32).to_le_bytes());
        gtp.extend_from_slice(data);
    }
    assert!(gtp.len() <= PAGE as usize);
    gtp.resize(PAGE as usize, 0);

    // ---- tile set
    let mut d = vec![0u8; 0xC0];
    let put32 =
        |d: &mut Vec<u8>, at: usize, v: u32| d[at..at + 4].copy_from_slice(&v.to_le_bytes());
    let put64 =
        |d: &mut Vec<u8>, at: usize, v: u64| d[at..at + 8].copy_from_slice(&v.to_le_bytes());
    d[0..4].copy_from_slice(b"GRPG");
    put32(&mut d, 4, 5);
    put32(&mut d, 0x1C, 2);
    put32(&mut d, 0x28, 2);
    put32(&mut d, 0x34, TILE);
    put32(&mut d, 0x38, TILE);
    put32(&mut d, 0x3C, BORDER);
    put32(&mut d, 0x80, PAGE);

    // layers
    here64(&mut d, 0x20);
    for dt in [1u32, 8] {
        d.extend_from_slice(&dt.to_le_bytes());
        d.extend_from_slice(&u32::MAX.to_le_bytes());
    }
    // level tile indices (layer-interleaved), flat tiles: 0=A, 1=B(uniform), 2=C, 3=D
    let fb = 0x8000_0000u32;
    let l0 = [0u32, 3 | fb, 1, 3 | fb];
    let l1 = [2u32, 3];
    let l0_at = d.len();
    l0.iter()
        .for_each(|v| d.extend_from_slice(&v.to_le_bytes()));
    let l1_at = d.len();
    l1.iter()
        .for_each(|v| d.extend_from_slice(&v.to_le_bytes()));
    here64(&mut d, 0x2C);
    for (w, h, at) in [(2u32, 1u32, l0_at), (1, 1, l1_at)] {
        d.extend_from_slice(&w.to_le_bytes());
        d.extend_from_slice(&h.to_le_bytes());
        d.extend_from_slice(&(at as u64).to_le_bytes());
    }

    // parameter blocks: 0 = BC3 raw, 1 = BC3 lz77, 2 = uniform
    let bc_block = |comp: &str, compressor: &str| {
        let mut b = Vec::new();
        b.extend_from_slice(&0x238eu16.to_le_bytes());
        let mut n = [0u8; 16];
        n[..comp.len()].copy_from_slice(comp.as_bytes());
        b.extend_from_slice(&n);
        let mut n = [0u8; 16];
        n[..compressor.len()].copy_from_slice(compressor.as_bytes());
        b.extend_from_slice(&n);
        b.extend_from_slice(&[0; 4]);
        b.extend_from_slice(&[0, 0, 0, 1]); // data type R8G8B8A8_SRGB
        b.extend_from_slice(&[0; 2]);
        b.extend_from_slice(b"BC3 ");
        b.extend_from_slice(&[0, 1, 0, 0]); // embedded mip
        b.extend_from_slice(&[0; 4]);
        assert_eq!(b.len(), 0x38);
        b
    };
    let mut uniform = Vec::new();
    uniform.extend_from_slice(&0x42u16.to_le_bytes());
    uniform.extend_from_slice(&[0; 2]);
    for v in [4u32, 1, 1] {
        uniform.extend_from_slice(&v.to_le_bytes());
    }
    let blocks = [
        (0u32, 9u32, bc_block("raw", "raw")),
        (1, 9, bc_block("lz77", "fastlz0.1.0")),
        (2, 0, uniform),
    ];
    let mut block_at = Vec::new();
    for (_, _, b) in &blocks {
        block_at.push(d.len());
        d.extend_from_slice(b);
    }
    put32(&mut d, 0x9C, blocks.len() as u32);
    here64(&mut d, 0xA0);
    for ((id, codec, b), at) in blocks.iter().zip(&block_at) {
        d.extend_from_slice(&id.to_le_bytes());
        d.extend_from_slice(&codec.to_le_bytes());
        d.extend_from_slice(&(b.len() as u32).to_le_bytes());
        d.extend_from_slice(&(*at as u64).to_le_bytes());
    }

    // page files
    put32(&mut d, 0x84, 1);
    here64(&mut d, 0x88);
    let mut name = [0u8; 512];
    for (i, u) in "Test_abc.gtp".encode_utf16().enumerate() {
        name[2 * i..2 * i + 2].copy_from_slice(&u.to_le_bytes());
    }
    d.extend_from_slice(&name);
    d.extend_from_slice(&1u32.to_le_bytes());
    d.extend_from_slice(&[7; 16]);
    d.extend_from_slice(&2u32.to_le_bytes());

    // FourCC metadata
    let leaf = |tag: &[u8; 4], value| FourCcNode {
        fourcc: *tag,
        value,
    };
    let tree = vec![leaf(
        b"META",
        FourCcValue::Node(vec![leaf(
            b"ATLS",
            FourCcValue::Node(vec![leaf(
                b"TXTS",
                FourCcValue::Node(vec![leaf(
                    b"TXTR",
                    FourCcValue::Node(vec![
                        leaf(b"NAME", FourCcValue::String("ABC".into())),
                        leaf(b"WDTH", FourCcValue::U32(16)),
                        leaf(b"HGHT", FourCcValue::U32(8)),
                        leaf(b"XXXX", FourCcValue::U32(0)),
                        leaf(b"YYYY", FourCcValue::U32(0)),
                        leaf(b"ADDR", FourCcValue::String("None".into())),
                    ]),
                )]),
            )]),
        ), leaf(b"PROJ", FourCcValue::String(r#"<Project><LayerConfig><LayerDescription Name="Albedo" /><LayerDescription Name="Normal" /></LayerConfig></Project>"#.into()))]),
    )];
    let at = d.len();
    let mut fc = Vec::new();
    fourcc::write(&tree, at, &mut fc);
    put64(&mut d, 0x94, at as u64);
    put32(&mut d, 0x90, fc.len() as u32);
    d.extend_from_slice(&fc);

    // packed tile IDs and flat tiles
    let packed = [
        PackedTileId::new(0, 0, 0, 0),
        PackedTileId::new(0, 0, 1, 0),
        PackedTileId::new(0, 1, 0, 0),
        PackedTileId::new(1, 1, 0, 0),
    ];
    put32(&mut d, 0x58, packed.len() as u32);
    here64(&mut d, 0x5C);
    packed
        .iter()
        .for_each(|p| d.extend_from_slice(&p.0.to_le_bytes()));
    put32(&mut d, 0x44, 4);
    here64(&mut d, 0x48);
    for i in 0..4u16 {
        for v in [0u16, 0, i, 1] {
            d.extend_from_slice(&v.to_le_bytes());
        }
        d.extend_from_slice(&(i as u32).to_le_bytes());
    }

    Fixture {
        gts: d,
        pages: HashMap::from([("Test_abc.gtp".to_owned(), gtp)]),
    }
}

#[test]
fn fourcc_round_trip() {
    let f = fixture();
    let ts = TileSet::parse(&f.gts).unwrap();
    let mut again = Vec::new();
    fourcc::write(&ts.fourcc, 0, &mut again);
    assert_eq!(
        fourcc::parse(&again, 0, again.len() as u64).unwrap(),
        ts.fourcc
    );
}

#[test]
fn parse_synthetic_tile_set() {
    let f = fixture();
    let ts = TileSet::parse(&f.gts).unwrap();
    assert_eq!(ts.layers.len(), 2);
    assert!(ts.layers[0].data_type.is_srgb());
    assert_eq!(ts.layers[1].data_type, DataType::X8Y8Z8W8);
    assert_eq!((ts.content_width(), ts.content_height()), (8, 8));
    assert_eq!(ts.levels[0].width, 2);
    assert_eq!(ts.page_files[0].name, "Test_abc.gtp");
    assert_eq!(ts.textures.len(), 1);
    let tex = ts.texture("abc").unwrap();
    assert_eq!((tex.width, tex.height), (16, 8));
    assert_eq!(ts.tile(0, 0, 1, 0), Some(TileRef::Stored(1)));
    assert_eq!(ts.tile(0, 1, 0, 0), Some(TileRef::Fallback(3)));
    assert_eq!(ts.tile(0, 0, 2, 0), None);
    assert_eq!(ts.flat_tiles[2].packed_tile, 2);
    assert_eq!(ts.packed_tiles[3].layer(), 1);
    assert_eq!(ts.packed_tiles[3].level(), 1);
    match &ts.parameter_block(1).unwrap().kind {
        ParameterKind::Bc(p) => {
            assert_eq!(p.compression, "lz77");
            assert_eq!(p.format, Some(BcFormat::Bc3));
            assert!(p.embedded_mip);
        }
        k => panic!("{k:?}"),
    }
    assert_eq!(ts.bc_format(0), Some(BcFormat::Bc3));
    assert_eq!(ts.layer_names(), ["Albedo", "Normal"]);
    assert_eq!(ts.finest_stored_level(tex, 0), Some(0));
    assert_eq!(ts.finest_stored_level(tex, 1), Some(1));
}

#[test]
fn page_file_chunks() {
    let f = fixture();
    let ts = TileSet::parse(&f.gts).unwrap();
    let gtp = PageFile::parse(&f.pages["Test_abc.gtp"], ts.page_size).unwrap();
    assert_eq!(gtp.num_pages(), 1);
    assert_eq!(gtp.chunk_offsets(0).unwrap().len(), 4);
    let c = gtp.chunk(0, 1).unwrap();
    assert_eq!(c.codec, Codec::Uniform);
    assert_eq!(
        ts.decode_chunk(&c).unwrap(),
        TileData::Uniform([200, 100, 50, 255])
    );
    let c = gtp.chunk(0, 2).unwrap();
    match ts.decode_chunk(&c).unwrap() {
        TileData::Bc { format, data } => {
            assert_eq!(format, BcFormat::Bc3);
            assert_eq!(data, bc3_tile(30));
        }
        t => panic!("{t:?}"),
    }
    assert!(gtp.chunk(0, 4).is_err());
    assert!(gtp.chunk(1, 0).is_err());
}

#[test]
fn extract_bc_strips_borders_and_stitches() {
    let f = fixture();
    let ts = TileSet::parse(&f.gts).unwrap();
    let tex = ts.texture("ABC").unwrap().clone();
    let img = ts.extract(&f.pages, &tex, 0, 0, OutputFormat::Bc).unwrap();
    assert_eq!((img.width, img.height), (16, 8));
    assert_eq!(img.format, ImageFormat::Bc(BcFormat::Bc3));
    assert!(img.srgb);
    // 4×2 blocks: left two columns from tile A's content blocks (1..=2, 1..=2), right two solid.
    let block = |bx: usize, by: usize| &img.data[(by * 4 + bx) * 16..(by * 4 + bx + 1) * 16];
    for by in 0..2 {
        for bx in 0..2 {
            let want = BcFormat::Bc3.solid_block(block_color(10, bx as u32 + 1, by as u32 + 1));
            assert_eq!(block(bx, by), &want[..]);
        }
        for bx in 2..4 {
            assert_eq!(
                block(bx, by),
                &BcFormat::Bc3.solid_block([200, 100, 50, 255])[..]
            );
        }
    }
}

#[test]
fn extract_rgba_and_levels() {
    let f = fixture();
    let ts = TileSet::parse(&f.gts).unwrap();
    let tex = ts.texture("abc").unwrap().clone();
    let img = ts
        .extract(&f.pages, &tex, 0, 0, OutputFormat::Rgba8)
        .unwrap();
    assert_eq!(img.format, ImageFormat::Rgba8);
    assert_eq!(img.data.len(), 16 * 8 * 4);
    let px = |x: usize, y: usize| &img.data[(y * 16 + x) * 4..(y * 16 + x) * 4 + 4];
    let expect = BcFormat::Bc3.decode_block(&BcFormat::Bc3.solid_block(block_color(10, 1, 1)))[0];
    assert_eq!(px(0, 0), &expect);
    // Uniform tiles are written exactly in RGBA output (no RGB565 round trip).
    assert_eq!(px(15, 7), &[200, 100, 50, 255]);
    assert_eq!(img.to_rgba8(), img.data);

    // Level 1: the 16×8 texture is 8×4 inside one FastLZ tile.
    let l1 = ts.extract(&f.pages, &tex, 0, 1, OutputFormat::Bc).unwrap();
    assert_eq!((l1.width, l1.height), (8, 4));
    assert_eq!(l1.data.len(), 2 * 16);
    assert_eq!(
        &l1.data[..16],
        &BcFormat::Bc3.solid_block(block_color(30, 1, 1))[..]
    );

    // Layer 1 is only stored at level 1: BC output refuses, RGBA output upsamples.
    assert!(matches!(
        ts.extract(&f.pages, &tex, 1, 0, OutputFormat::Bc),
        Err(Error::NotFound(_))
    ));
    assert_eq!(ts.fallback_source(0, 1, 1, 0), Some((1, 0, 0, 3)));
    assert_eq!(ts.fallback_source(0, 0, 1, 0), Some((0, 1, 0, 1)));
    let up = ts
        .extract(&f.pages, &tex, 1, 0, OutputFormat::Rgba8)
        .unwrap();
    let decode = |c| BcFormat::Bc3.decode_block(&BcFormat::Bc3.solid_block(c))[0];
    let upx = |x: usize, y: usize| &up.data[(y * 16 + x) * 4..(y * 16 + x) * 4 + 4];
    // Level-0 pixel (x, y) → level-1 content pixel (x/2, y/2) → tile pixel +4 border.
    assert_eq!(upx(15, 7), &decode(block_color(40, 2, 1)));
    assert_eq!(upx(0, 0), &decode(block_color(40, 1, 1)));
    assert_eq!(upx(8, 0), &decode(block_color(40, 2, 1)));
    let mips = ts
        .extract_mips(&f.pages, &tex, 1, 0, OutputFormat::Bc)
        .unwrap();
    assert_eq!(mips.len(), 1);
    assert_eq!((mips[0].width, mips[0].height), (8, 4));
    let mips = ts
        .extract_mips(&f.pages, &tex, 0, 0, OutputFormat::Bc)
        .unwrap();
    assert_eq!(mips.len(), 2);
}

#[test]
fn rejects_garbage() {
    assert!(TileSet::parse(b"nope").is_err());
    let mut f = fixture();
    f.gts[4] = 6;
    assert!(matches!(TileSet::parse(&f.gts), Err(Error::Unsupported(_))));
    let f = fixture();
    // Truncation anywhere in the tables must fail cleanly, never panic.
    for cut in (0xC0..f.gts.len()).step_by(7) {
        let _ = TileSet::parse(&f.gts[..cut]);
    }
    assert!(PageFile::parse(b"GRAP", 4096).is_err());
}

#[test]
fn layer_config_paths() {
    assert_eq!(
        LayerConfig::from_raw(3)
            .tile_set_path("20FA472C")
            .as_deref(),
        Some("Generated/Public/VirtualTextures/Albedo_Normal_Physical_2.gts")
    );
    assert_eq!(LayerConfig::from_raw(7).tile_set_path("2"), None);
}

#[test]
fn bank_nodes() {
    use bg3_lsf::{Node, Value};
    let res = Node::new("Resource")
        .with_attr("ID", Value::FixedString("vt-1".into()))
        .with_attr("Name", Value::LSString("Floor".into()))
        .with_attr("GTexFileName", Value::FixedString("abc".into()))
        .with_attr("VirtualTextureLayerConfig", Value::UInt(3))
        .with_attr("PrefetchMipLevel", Value::Int8(-1));
    let vt = VirtualTextureResource::from_node(&res).unwrap();
    assert_eq!(vt.gtex_file_name, "abc");
    assert_eq!(vt.layer_config, 3);
    assert_eq!(vt.prefetch_mip_level, -1);

    let mat = Node::new("Resource").with_child(
        Node::new("VirtualTextureParameters")
            .with_attr("ID", Value::FixedString("vt-1".into()))
            .with_attr("Index", Value::Int(0))
            .with_attr("ParameterName", Value::FixedString("virtualtexture".into()))
            .with_attr("Enabled", Value::Bool(true)),
    );
    let p = material_virtual_textures(&mat);
    assert_eq!(p.len(), 1);
    assert_eq!(p[0].id, "vt-1");
    assert_eq!(p[0].parameter_name, "virtualtexture");
}
