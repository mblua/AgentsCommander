use super::{ManifestPathEncoding, ResourceBoundKind, SeedManifestError, MAX_FIELD_BYTES};
use std::path::{Component, Path};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DecodedPathComponents {
    Utf8(Vec<String>),
    UnixBytes(Vec<Vec<u8>>),
    WindowsUtf16(Vec<Vec<u16>>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ManifestPathIdentity {
    pub(crate) encoding: ManifestPathEncoding,
    pub(crate) serialized: String,
    pub(crate) components: DecodedPathComponents,
}

impl ManifestPathIdentity {
    pub(crate) fn parse(
        encoding: ManifestPathEncoding,
        serialized: String,
    ) -> Result<Self, SeedManifestError> {
        if serialized.len() > MAX_FIELD_BYTES {
            return Err(SeedManifestError::resource_bound(
                ResourceBoundKind::PathBytes,
                MAX_FIELD_BYTES,
                serialized.len(),
            ));
        }

        let components = match encoding {
            ManifestPathEncoding::Utf8 => {
                DecodedPathComponents::Utf8(parse_utf8_components(&serialized)?)
            }
            ManifestPathEncoding::UnixBytesHex => {
                DecodedPathComponents::UnixBytes(parse_unix_hex_components(&serialized)?)
            }
            ManifestPathEncoding::WindowsUtf16Hex => {
                DecodedPathComponents::WindowsUtf16(parse_windows_hex_components(&serialized)?)
            }
        };
        let identity = Self {
            encoding,
            serialized,
            components,
        };
        identity.require_ac_root()?;
        Ok(identity)
    }

    pub(crate) fn from_relative_path(path: &Path) -> Result<Self, SeedManifestError> {
        if path.is_absolute() {
            return Err(SeedManifestError::Validation(format!(
                "manifest path must be relative: {}",
                path.display()
            )));
        }

        let mut components = Vec::new();
        for component in path.components() {
            match component {
                Component::Normal(value) => components.push(value),
                _ => {
                    return Err(SeedManifestError::Validation(format!(
                        "manifest path contains a non-normal component: {}",
                        path.display()
                    )))
                }
            }
        }
        if components.is_empty() {
            return Err(SeedManifestError::Validation(
                "manifest path must contain at least one component".to_string(),
            ));
        }

        if components
            .iter()
            .all(|component| component.to_str().is_some())
        {
            let serialized = components
                .iter()
                .map(|component| component.to_str().unwrap_or_default())
                .collect::<Vec<_>>()
                .join("/");
            return Self::parse(ManifestPathEncoding::Utf8, serialized);
        }

        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;

            let mut bytes = Vec::new();
            for (index, component) in components.iter().enumerate() {
                if index > 0 {
                    bytes.push(b'/');
                }
                bytes.extend_from_slice(component.as_bytes());
            }
            let serialized = encode_lower_hex_bytes(&bytes);
            return Self::parse(ManifestPathEncoding::UnixBytesHex, serialized);
        }

        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt;

            let mut units = Vec::new();
            for (index, component) in components.iter().enumerate() {
                if index > 0 {
                    units.push(u16::from(b'/'));
                }
                units.extend(component.encode_wide());
            }
            let serialized = encode_lower_hex_u16(&units);
            return Self::parse(ManifestPathEncoding::WindowsUtf16Hex, serialized);
        }

        #[allow(unreachable_code)]
        Err(SeedManifestError::Validation(
            "native path encoding is unsupported on this platform".to_string(),
        ))
    }

    pub(crate) fn encoding(&self) -> ManifestPathEncoding {
        self.encoding
    }

    pub(crate) fn serialized(&self) -> &str {
        &self.serialized
    }

    pub(crate) fn key(&self) -> ManifestPathKey {
        ManifestPathKey {
            encoding: self.encoding,
            serialized: self.serialized.clone(),
        }
    }

    pub(crate) fn require_ac_root(&self) -> Result<(), SeedManifestError> {
        let is_ac = match &self.components {
            DecodedPathComponents::Utf8(components) => {
                components.first().map(String::as_str) == Some(".ac")
            }
            DecodedPathComponents::UnixBytes(components) => {
                components.first().map(Vec::as_slice) == Some(b".ac".as_slice())
            }
            DecodedPathComponents::WindowsUtf16(components) => {
                components.first().map(|component| component.as_slice())
                    == Some([46_u16, 97_u16, 99_u16].as_slice())
            }
        };
        if !is_ac {
            return Err(SeedManifestError::Validation(format!(
                "manifest path must begin with the .ac component: {}",
                self.serialized
            )));
        }
        Ok(())
    }

    pub(crate) fn starts_with_utf8_components(&self, prefix: &[String]) -> bool {
        match &self.components {
            DecodedPathComponents::Utf8(components) => components.starts_with(prefix),
            DecodedPathComponents::UnixBytes(components) => {
                components.len() >= prefix.len()
                    && components
                        .iter()
                        .zip(prefix)
                        .all(|(actual, expected)| actual.as_slice() == expected.as_bytes())
            }
            DecodedPathComponents::WindowsUtf16(components) => {
                components.len() >= prefix.len()
                    && components.iter().zip(prefix).all(|(actual, expected)| {
                        actual.iter().copied().eq(expected.encode_utf16())
                    })
            }
        }
    }

    pub(crate) fn is_strict_descendant_of_utf8(&self, prefix: &[String]) -> bool {
        let component_count = match &self.components {
            DecodedPathComponents::Utf8(components) => components.len(),
            DecodedPathComponents::UnixBytes(components) => components.len(),
            DecodedPathComponents::WindowsUtf16(components) => components.len(),
        };
        component_count > prefix.len() && self.starts_with_utf8_components(prefix)
    }

    pub(crate) fn utf8_component_equals(&self, index: usize, expected: &str) -> bool {
        match &self.components {
            DecodedPathComponents::Utf8(components) => {
                components.get(index).map(String::as_str) == Some(expected)
            }
            DecodedPathComponents::UnixBytes(components) => {
                components.get(index).map(Vec::as_slice) == Some(expected.as_bytes())
            }
            DecodedPathComponents::WindowsUtf16(components) => components
                .get(index)
                .map(|actual| actual.iter().copied().eq(expected.encode_utf16()))
                .unwrap_or(false),
        }
    }
}

pub(crate) fn validate_plain_component(
    component: &str,
    label: &str,
) -> Result<(), SeedManifestError> {
    if component.is_empty() || component == "." || component == ".." {
        return Err(SeedManifestError::Validation(format!(
            "{} contains an empty or traversal component",
            label
        )));
    }
    if component.contains('/') || component.contains('\0') {
        return Err(SeedManifestError::Validation(format!(
            "{} contains a separator or NUL inside a component",
            label
        )));
    }
    Ok(())
}

pub(crate) fn parse_utf8_components(serialized: &str) -> Result<Vec<String>, SeedManifestError> {
    if serialized.is_empty() || serialized.starts_with('/') || serialized.ends_with('/') {
        return Err(SeedManifestError::Validation(format!(
            "invalid relative UTF-8 manifest path: {serialized:?}"
        )));
    }
    let components = serialized
        .split('/')
        .map(str::to_string)
        .collect::<Vec<_>>();
    for component in &components {
        validate_plain_component(component, "UTF-8 manifest path")?;
    }
    Ok(components)
}

pub(crate) fn parse_lower_hex_bytes(serialized: &str) -> Result<Vec<u8>, SeedManifestError> {
    if serialized.is_empty() || !serialized.len().is_multiple_of(2) {
        return Err(SeedManifestError::Validation(
            "unix_bytes_hex must contain a nonempty even number of digits".to_string(),
        ));
    }
    if !serialized
        .bytes()
        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(SeedManifestError::Validation(
            "unix_bytes_hex must use lowercase fixed-width hexadecimal".to_string(),
        ));
    }
    serialized
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|digits| {
            let text = std::str::from_utf8(digits).map_err(|error| {
                SeedManifestError::Validation(format!("invalid unix_bytes_hex: {error}"))
            })?;
            u8::from_str_radix(text, 16).map_err(|error| {
                SeedManifestError::Validation(format!("invalid unix_bytes_hex: {error}"))
            })
        })
        .collect()
}

pub(crate) fn parse_unix_hex_components(
    serialized: &str,
) -> Result<Vec<Vec<u8>>, SeedManifestError> {
    let bytes = parse_lower_hex_bytes(serialized)?;
    if std::str::from_utf8(&bytes).is_ok() {
        return Err(SeedManifestError::Validation(
            "unix_bytes_hex is noncanonical because the decoded path is valid UTF-8".to_string(),
        ));
    }
    split_byte_components(&bytes)
}

pub(crate) fn split_byte_components(bytes: &[u8]) -> Result<Vec<Vec<u8>>, SeedManifestError> {
    let mut components = Vec::new();
    for component in bytes.split(|byte| *byte == b'/') {
        if component.is_empty() || component == b"." || component == b".." || component.contains(&0)
        {
            return Err(SeedManifestError::Validation(
                "unix_bytes_hex contains an empty, traversal, or NUL component".to_string(),
            ));
        }
        components.push(component.to_vec());
    }
    Ok(components)
}

pub(crate) fn parse_windows_hex_components(
    serialized: &str,
) -> Result<Vec<Vec<u16>>, SeedManifestError> {
    if serialized.is_empty() || !serialized.len().is_multiple_of(4) {
        return Err(SeedManifestError::Validation(
            "windows_utf16_hex must contain a nonempty multiple of four digits".to_string(),
        ));
    }
    if !serialized
        .bytes()
        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(SeedManifestError::Validation(
            "windows_utf16_hex must use lowercase fixed-width hexadecimal".to_string(),
        ));
    }
    let units = serialized
        .as_bytes()
        .as_chunks::<4>()
        .0
        .iter()
        .map(|digits| {
            let text = std::str::from_utf8(digits).map_err(|error| {
                SeedManifestError::Validation(format!("invalid windows_utf16_hex: {error}"))
            })?;
            u16::from_str_radix(text, 16).map_err(|error| {
                SeedManifestError::Validation(format!("invalid windows_utf16_hex: {error}"))
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    if !char::decode_utf16(units.iter().copied()).any(|unit| unit.is_err()) {
        return Err(SeedManifestError::Validation(
            "windows_utf16_hex is noncanonical because the decoded path is valid Unicode"
                .to_string(),
        ));
    }

    let mut components = Vec::new();
    for component in units.split(|unit| *unit == u16::from(b'/')) {
        if component.is_empty()
            || component == [u16::from(b'.')]
            || component == [u16::from(b'.'), u16::from(b'.')]
            || component.contains(&0)
            || component.contains(&u16::from(b'\\'))
        {
            return Err(SeedManifestError::Validation(
                "windows_utf16_hex contains an empty, traversal, separator, or NUL component"
                    .to_string(),
            ));
        }
        components.push(component.to_vec());
    }
    Ok(components)
}

pub(crate) fn encode_lower_hex_bytes(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

pub(crate) fn encode_lower_hex_u16(units: &[u16]) -> String {
    let mut encoded = String::with_capacity(units.len().saturating_mul(4));
    for unit in units {
        use std::fmt::Write as _;
        let _ = write!(encoded, "{unit:04x}");
    }
    encoded
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ManifestPathKey {
    pub(crate) encoding: ManifestPathEncoding,
    pub(crate) serialized: String,
}

impl Ord for ManifestPathKey {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.encoding
            .as_str()
            .as_bytes()
            .cmp(other.encoding.as_str().as_bytes())
            .then_with(|| self.serialized.as_bytes().cmp(other.serialized.as_bytes()))
    }
}

impl PartialOrd for ManifestPathKey {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
