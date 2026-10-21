//! Material parameters as stored in shader graphs and `MaterialBank` instances.

use bg3_lsf::Node;

/// The value of one material parameter.
#[derive(Debug, Clone, PartialEq)]
pub enum ParamValue {
    Scalar(f32),
    /// `Vector3Parameters` / `VectorParameters` (3 or 4 components).
    Vector(Vec<f32>),
    /// `Texture2DParameters`: an ID in the `TextureBank`.
    Texture(String),
    /// `VirtualTextureParameters`: an ID in the `VirtualTextureBank`.
    VirtualTexture(String),
}

/// One parameter: `ScalarParameters`, `Vector3Parameters`, `Texture2DParameters`, ….
#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    pub name: String,
    pub value: ParamValue,
    /// In an instance: whether it overrides the graph default. In a graph: always `false`.
    pub enabled: bool,
    /// In a graph: the depth-only pass (`StaticDepth`) binds this parameter in its pixel shader,
    /// i.e. the graph samples it to decide coverage (alpha testing).
    pub depth_pass: bool,
}

/// Read every parameter node among `node`'s children.
///
/// Texture parameters without a `ParameterName` (unnamed graph inputs) keep an empty name.
pub fn read_params(node: &Node) -> Vec<Param> {
    node.children.iter().filter_map(read_param).collect()
}

fn read_param(c: &Node) -> Option<Param> {
    let name = c.attr_str("ParameterName").unwrap_or_default().to_owned();
    let enabled = c.attr("Enabled").and_then(|v| v.as_bool()).unwrap_or(false);
    let id = || {
        c.attr("ID")
            .map(|v| v.to_string())
            .filter(|s| !s.is_empty())
    };
    let value = match c.name.as_str() {
        "ScalarParameters" => ParamValue::Scalar(c.attr("Value")?.as_f64()? as f32),
        "Vector3Parameters" | "VectorParameters" | "Vector4Parameters" => ParamValue::Vector(
            c.attr("Value")?
                .components()?
                .into_iter()
                .map(|x| x as f32)
                .collect(),
        ),
        "Texture2DParameters" => ParamValue::Texture(id()?),
        "VirtualTextureParameters" => ParamValue::VirtualTexture(id()?),
        _ => return None,
    };
    Some(Param {
        name,
        value,
        enabled,
        depth_pass: c.child("StaticDepth").is_some_and(|d| {
            d.attr("DxPsIndex")
                .and_then(|v| v.as_i64())
                .is_some_and(|i| i >= 0)
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use bg3_lsf::Value;

    fn param(kind: &str, name: &str, enabled: bool, value: Value) -> Node {
        let key = if kind.starts_with("Texture") || kind.starts_with("Virtual") {
            "ID"
        } else {
            "Value"
        };
        Node::new(kind)
            .with_attr("ParameterName", Value::FixedString(name.into()))
            .with_attr("Enabled", Value::Bool(enabled))
            .with_attr(key, value)
    }

    #[test]
    fn reads_all_parameter_kinds() {
        let node = Node::new("Resource")
            .with_child(param("ScalarParameters", "Rough", true, Value::Float(0.25)))
            .with_child(param(
                "Vector3Parameters",
                "Tint",
                false,
                Value::Vec3([1.0, 0.5, 0.0]),
            ))
            .with_child(param(
                "Texture2DParameters",
                "basecolor",
                false,
                Value::FixedString("tex-1".into()),
            ))
            .with_child(param(
                "VirtualTextureParameters",
                "virtualtexture",
                true,
                Value::FixedString("vt-1".into()),
            ))
            .with_child(Node::new("SamplerStateParameters"));
        let params = read_params(&node);
        assert_eq!(params.len(), 4);
        assert_eq!(params[0].value, ParamValue::Scalar(0.25));
        assert!(params[0].enabled);
        assert!(!params[2].depth_pass);
        assert_eq!(params[1].value, ParamValue::Vector(vec![1.0, 0.5, 0.0]));
        assert_eq!(params[2].value, ParamValue::Texture("tex-1".into()));
        assert_eq!(params[3].value, ParamValue::VirtualTexture("vt-1".into()));
    }

    #[test]
    fn depth_pass_binding() {
        let bound = |idx: i8| {
            param(
                "Texture2DParameters",
                "basecolor",
                false,
                Value::FixedString("t".into()),
            )
            .with_child(Node::new("StaticDepth").with_attr("DxPsIndex", Value::Int8(idx)))
        };
        let node = Node::new("Material")
            .with_child(bound(0))
            .with_child(bound(-1));
        let params = read_params(&node);
        assert!(params[0].depth_pass);
        assert!(!params[1].depth_pass);
    }

    #[test]
    fn empty_texture_id_is_skipped() {
        let node = Node::new("Resource").with_child(param(
            "Texture2DParameters",
            "x",
            true,
            Value::FixedString(String::new()),
        ));
        assert!(read_params(&node).is_empty());
    }
}
