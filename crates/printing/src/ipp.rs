//! The few pieces of IPP/2.0 (RFC 8010) OpusDM needs: encode a request with
//! operation attributes, decode a response into its attribute groups.

/// `Get-Jobs`.
pub const OP_GET_JOBS: u16 = 0x000A;
/// `CUPS-Get-Default`.
pub const OP_CUPS_GET_DEFAULT: u16 = 0x4001;
/// `CUPS-Get-Printers`.
pub const OP_CUPS_GET_PRINTERS: u16 = 0x4002;
/// Group delimiter of the operation attributes.
pub const TAG_OPERATION: u8 = 0x01;
/// Group delimiter of the attributes of one job.
pub const TAG_JOB: u8 = 0x02;
/// End of the attributes.
pub const TAG_END: u8 = 0x03;
/// Group delimiter of the attributes of one printer.
pub const TAG_PRINTER: u8 = 0x04;
/// `client-error-not-found`.
pub const STATUS_NOT_FOUND: u16 = 0x0406;

/// Value tags used here; text, name and keyword are public for the tests of other modules.
const TAG_INTEGER: u8 = 0x21;
const TAG_BOOLEAN: u8 = 0x22;
const TAG_ENUM: u8 = 0x23;
pub const TAG_TEXT: u8 = 0x41;
pub const TAG_NAME: u8 = 0x42;
pub const TAG_KEYWORD: u8 = 0x44;
const TAG_URI: u8 = 0x45;
const TAG_CHARSET: u8 = 0x47;
const TAG_LANGUAGE: u8 = 0x48;
/// Range of the tags whose value is a string.
const TEXT_TAGS: std::ops::RangeInclusive<u8> = 0x30..=0x4A;
/// Tags below this one delimit groups.
const FIRST_VALUE_TAG: u8 = 0x10;
/// Length of the response header (version, status, request id).
const HEADER_LEN: usize = 8;

/// One attribute value.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Integer(i32),
    Boolean(bool),
    Enum(i32),
    Text(String),
    /// Any other tag, with its raw bytes.
    Other(u8, Vec<u8>),
}

impl Value {
    /// The number of an integer or an enum.
    pub fn as_int(&self) -> Option<i32> {
        match self {
            Value::Integer(n) | Value::Enum(n) => Some(*n),
            _ => None,
        }
    }

    /// The value of a boolean.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Boolean(b) => Some(*b),
            _ => None,
        }
    }

    /// The text of a string value (text, name, keyword, uri, ...).
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Text(s) => Some(s),
            _ => None,
        }
    }
}

/// The attributes of one group (operation, job, printer), in order.
#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    pub tag: u8,
    pub attrs: Vec<(String, Vec<Value>)>,
}

impl Group {
    /// The values of the attribute `name`.
    pub fn get(&self, name: &str) -> Option<&[Value]> {
        self.attrs.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_slice())
    }

    /// The first value of `name` as text.
    pub fn str(&self, name: &str) -> Option<&str> {
        self.get(name)?.first()?.as_str()
    }

    /// The first value of `name` as a number.
    pub fn int(&self, name: &str) -> Option<i32> {
        self.get(name)?.first()?.as_int()
    }

    /// The first value of `name` as a boolean.
    pub fn bool(&self, name: &str) -> Option<bool> {
        self.get(name)?.first()?.as_bool()
    }

    /// Every text value of `name`.
    pub fn strs(&self, name: &str) -> Vec<String> {
        self.get(name).map(|v| v.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default()
    }
}

/// A decoded response.
#[derive(Clone, Debug, PartialEq)]
pub struct Response {
    pub status: u16,
    pub groups: Vec<Group>,
}

impl Response {
    /// The groups with the delimiter `tag`.
    pub fn groups_of(&self, tag: u8) -> impl Iterator<Item = &Group> {
        self.groups.iter().filter(move |g| g.tag == tag)
    }

    /// The `status-message` of the operation group.
    pub fn status_message(&self) -> Option<&str> {
        self.groups_of(TAG_OPERATION).find_map(|g| g.str("status-message"))
    }

    /// Whether the status is a success (`successful-ok*`).
    pub fn is_ok(&self) -> bool {
        self.status < 0x0100
    }
}

/// A request: the operation and its operation attributes.
#[derive(Clone, Debug)]
pub struct Request {
    op: u16,
    id: u32,
    attrs: Vec<(u8, String, Vec<Vec<u8>>)>,
}

impl Request {
    /// A request for `op`, with the charset and the language CUPS needs.
    pub fn new(op: u16) -> Self {
        let r = Self { op, id: 1, attrs: Vec::new() };
        r.with(TAG_CHARSET, "attributes-charset", vec![b"utf-8".to_vec()]).with(TAG_LANGUAGE, "attributes-natural-language", vec![b"en".to_vec()])
    }

    /// Add an attribute with its tag and values.
    fn with(mut self, tag: u8, name: &str, values: Vec<Vec<u8>>) -> Self {
        self.attrs.push((tag, name.to_string(), values));
        self
    }

    /// Add a uri attribute.
    pub fn uri(self, name: &str, value: &str) -> Self {
        self.with(TAG_URI, name, vec![value.as_bytes().to_vec()])
    }

    /// Add a name attribute.
    pub fn name(self, name: &str, value: &str) -> Self {
        self.with(TAG_NAME, name, vec![value.as_bytes().to_vec()])
    }

    /// Add a keyword attribute.
    pub fn keyword(self, name: &str, value: &str) -> Self {
        self.with(TAG_KEYWORD, name, vec![value.as_bytes().to_vec()])
    }

    /// Add a 1setOf keyword attribute.
    pub fn keywords(self, name: &str, values: &[&str]) -> Self {
        self.with(TAG_KEYWORD, name, values.iter().map(|v| v.as_bytes().to_vec()).collect())
    }

    /// Add a boolean attribute.
    pub fn boolean(self, name: &str, value: bool) -> Self {
        self.with(TAG_BOOLEAN, name, vec![vec![u8::from(value)]])
    }

    /// Add an integer attribute.
    pub fn integer(self, name: &str, value: i32) -> Self {
        self.with(TAG_INTEGER, name, vec![value.to_be_bytes().to_vec()])
    }

    /// The bytes of the request.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = vec![2, 0];
        out.extend_from_slice(&self.op.to_be_bytes());
        out.extend_from_slice(&self.id.to_be_bytes());
        out.push(TAG_OPERATION);
        for (tag, name, values) in &self.attrs {
            for (i, value) in values.iter().enumerate() {
                let n = if i == 0 { name.as_str() } else { "" };
                out.push(*tag);
                _push_sized(&mut out, n.as_bytes());
                _push_sized(&mut out, value);
            }
        }
        out.push(TAG_END);
        out
    }
}

/// Append `bytes` after their length as a big-endian u16.
fn _push_sized(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u16).to_be_bytes());
    out.extend_from_slice(bytes);
}

/// Reads the response bytes in order.
struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    /// The next `n` bytes.
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self.pos.checked_add(n).filter(|e| *e <= self.bytes.len()).ok_or_else(|| format!("IPP response cut at byte {}", self.pos))?;
        let slice = &self.bytes[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    /// A big-endian u16.
    fn u16(&mut self) -> Result<u16, String> {
        let b = self.take(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }
}

/// Decode a response.
pub fn decode(bytes: &[u8]) -> Result<Response, String> {
    if bytes.len() < HEADER_LEN { return Err(format!("IPP response too short ({} bytes)", bytes.len())); }
    let status = u16::from_be_bytes([bytes[2], bytes[3]]);
    let mut c = Cursor { bytes, pos: HEADER_LEN };
    let mut groups: Vec<Group> = Vec::new();
    loop {
        let tag = c.take(1)?[0];
        if tag == TAG_END { break; }
        if tag < FIRST_VALUE_TAG {
            groups.push(Group { tag, attrs: Vec::new() });
            continue;
        }
        let name_len = c.u16()? as usize;
        let name = String::from_utf8_lossy(c.take(name_len)?).into_owned();
        let value_len = c.u16()? as usize;
        let value = _value(tag, c.take(value_len)?);
        let group = groups.last_mut().ok_or("IPP attribute outside a group")?;
        if name.is_empty() {
            let last = group.attrs.last_mut().ok_or("IPP additional value without an attribute")?;
            last.1.push(value);
        } else {
            group.attrs.push((name, vec![value]));
        }
    }
    Ok(Response { status, groups })
}

/// The value of `tag` held in `raw`.
fn _value(tag: u8, raw: &[u8]) -> Value {
    match tag {
        TAG_INTEGER if raw.len() == 4 => Value::Integer(i32::from_be_bytes([raw[0], raw[1], raw[2], raw[3]])),
        TAG_ENUM if raw.len() == 4 => Value::Enum(i32::from_be_bytes([raw[0], raw[1], raw[2], raw[3]])),
        TAG_BOOLEAN if raw.len() == 1 => Value::Boolean(raw[0] != 0),
        t if TEXT_TAGS.contains(&t) => Value::Text(String::from_utf8_lossy(raw).into_owned()),
        t => Value::Other(t, raw.to_vec()),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Builds response bytes for the tests.
    pub(crate) struct Fixture(pub Vec<u8>);

    impl Fixture {
        /// A response header with `status`.
        pub(crate) fn new(status: u16) -> Self {
            let mut v = vec![2, 0];
            v.extend_from_slice(&status.to_be_bytes());
            v.extend_from_slice(&1u32.to_be_bytes());
            Fixture(v)
        }

        /// Start a group.
        pub(crate) fn group(mut self, tag: u8) -> Self {
            self.0.push(tag);
            self
        }

        /// One attribute value (an empty name adds a value to the previous one).
        pub(crate) fn attr(mut self, tag: u8, name: &str, value: &[u8]) -> Self {
            self.0.push(tag);
            _push_sized(&mut self.0, name.as_bytes());
            _push_sized(&mut self.0, value);
            self
        }

        /// A text-like attribute.
        pub(crate) fn text(self, tag: u8, name: &str, value: &str) -> Self {
            self.attr(tag, name, value.as_bytes())
        }

        /// An enum attribute.
        pub(crate) fn enumv(self, name: &str, value: i32) -> Self {
            self.attr(TAG_ENUM, name, &value.to_be_bytes())
        }

        /// A boolean attribute.
        pub(crate) fn boolean(self, name: &str, value: bool) -> Self {
            self.attr(TAG_BOOLEAN, name, &[u8::from(value)])
        }

        /// End the response.
        pub(crate) fn end(mut self) -> Vec<u8> {
            self.0.push(TAG_END);
            self.0
        }
    }

    /// Whether `needle` appears in `hay`.
    fn contains(hay: &[u8], needle: &[u8]) -> bool {
        hay.windows(needle.len()).any(|w| w == needle)
    }

    /// The two printers seen on the owner's machine.
    pub(crate) fn printers_fixture() -> Vec<u8> {
        Fixture::new(0)
            .group(TAG_OPERATION)
            .text(TAG_CHARSET, "attributes-charset", "utf-8")
            .group(TAG_PRINTER)
            .text(TAG_NAME, "printer-name", "C364Series")
            .enumv("printer-state", 3)
            .boolean("printer-is-accepting-jobs", true)
            .text(TAG_KEYWORD, "printer-state-reasons", "media-empty-error")
            .text(TAG_KEYWORD, "", "toner-low-warning")
            .text(TAG_TEXT, "printer-info", "KONICA MINOLTA bizhub C224e(98:1F:7C)")
            .group(TAG_PRINTER)
            .text(TAG_NAME, "printer-name", "opusdm_probe")
            .end()
    }

    #[test]
    fn encodes_a_get_printers_request() {
        let bytes = Request::new(OP_CUPS_GET_PRINTERS).keywords("requested-attributes", &["printer-name", "printer-state"]).encode();
        assert_eq!(&bytes[..10], &[2, 0, 0x40, 0x02, 0, 0, 0, 1, 0x01, 0x47]);
        assert!(contains(&bytes, b"attributes-charset"));
        assert!(contains(&bytes, b"utf-8"));
        let mut second = vec![0x44, 0, 0, 0, 13];
        second.extend_from_slice(b"printer-state");
        assert!(contains(&bytes, &second), "the second value has an empty name");
        assert_eq!(*bytes.last().unwrap(), TAG_END);
    }

    #[test]
    fn decodes_printers_and_values() {
        let r = decode(&printers_fixture()).unwrap();
        assert!(r.is_ok());
        let printers: Vec<&Group> = r.groups_of(TAG_PRINTER).collect();
        assert_eq!(printers.len(), 2);
        assert_eq!(printers[0].str("printer-name"), Some("C364Series"));
        assert_eq!(printers[0].int("printer-state"), Some(3));
        assert_eq!(printers[0].bool("printer-is-accepting-jobs"), Some(true));
        assert_eq!(printers[0].strs("printer-state-reasons"), vec!["media-empty-error".to_string(), "toner-low-warning".to_string()]);
        assert_eq!(printers[1].str("printer-name"), Some("opusdm_probe"));
    }

    #[test]
    fn decode_rejects_truncated_input() {
        assert!(decode(&[2, 0, 0]).is_err());
        let full = printers_fixture();
        assert!(decode(&full[..full.len() - 10]).is_err());
    }

    #[test]
    fn status_message_is_read() {
        let bytes = Fixture::new(STATUS_NOT_FOUND).group(TAG_OPERATION).text(TAG_TEXT, "status-message", "No default printer.").end();
        let r = decode(&bytes).unwrap();
        assert_eq!(r.status, STATUS_NOT_FOUND);
        assert!(!r.is_ok());
        assert_eq!(r.status_message(), Some("No default printer."));
    }
}
