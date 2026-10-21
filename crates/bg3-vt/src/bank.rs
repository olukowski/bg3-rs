//! Resource-bank side: `VirtualTextureBank` resources and material virtual-texture parameters.

use bg3_lsf::Node;

/// A `VirtualTextureBank` → `Resource` node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VirtualTextureResource {
    pub id: String,
    pub name: String,
    /// Name of the texture inside the tile set (32 hex digits). Its first character selects the
    /// tile set file, e.g. `2…` → `Albedo_Normal_Physical_2.gts`.
    pub gtex_file_name: String,
    /// `VirtualTextureLayerConfig`: which family of tile sets (3 = albedo/normal/physical).
    pub layer_config: u32,
    pub referenced_color_spaces: u32,
    pub prefetch: bool,
    pub prefetch_mip_level: i8,
}

impl VirtualTextureResource {
    /// Read a `Resource` node of the `VirtualTextureBank` region. `None` without `ID`/`GTexFileName`.
    pub fn from_node(node: &Node) -> Option<Self> {
        let s = |n: &str| node.attr(n).map(|v| v.to_string());
        let i = |n: &str| node.attr(n).and_then(|v| v.as_i64());
        Some(Self {
            id: s("ID").filter(|s| !s.is_empty())?,
            name: s("Name").unwrap_or_default(),
            gtex_file_name: s("GTexFileName").filter(|s| !s.is_empty())?,
            layer_config: i("VirtualTextureLayerConfig").unwrap_or(3) as u32,
            referenced_color_spaces: i("ReferencedColorSpaces").unwrap_or(0) as u32,
            prefetch: node
                .attr("Prefetch")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            prefetch_mip_level: i("PrefetchMipLevel").unwrap_or(-1) as i8,
        })
    }

    /// Every resource of a bank file's `VirtualTextureBank` region.
    pub fn read_bank(res: &bg3_lsf::Resource) -> Vec<Self> {
        res.region("VirtualTextureBank")
            .into_iter()
            .flat_map(|n| n.children_named("Resource"))
            .filter_map(Self::from_node)
            .collect()
    }
}

/// A material's `VirtualTextureParameters` child.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaterialVirtualTexture {
    /// Shader parameter name (BG3 uses `"virtualtexture"`).
    pub parameter_name: String,
    /// ID of the [`VirtualTextureResource`].
    pub id: String,
    /// Virtual texture slot of the shader.
    pub index: i32,
    pub enabled: bool,
}

/// The virtual-texture parameters of a `MaterialBank` → `Resource` node.
pub fn material_virtual_textures(material: &Node) -> Vec<MaterialVirtualTexture> {
    material
        .children_named("VirtualTextureParameters")
        .filter_map(|c| {
            Some(MaterialVirtualTexture {
                parameter_name: c
                    .attr("ParameterName")
                    .map(|v| v.to_string())
                    .unwrap_or_default(),
                id: c
                    .attr("ID")
                    .map(|v| v.to_string())
                    .filter(|s| !s.is_empty())?,
                index: c.attr("Index").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
                enabled: c.attr("Enabled").and_then(|v| v.as_bool()).unwrap_or(true),
            })
        })
        .collect()
}
