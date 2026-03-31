/// Unit category derived from the sample unit segment of a Pyroscope profile type ID.
///
/// Profile type ID format: `<group>:<sample_type>:<sample_unit>:<period_type>:<period_unit>`
/// The 3rd segment (index 2) is the sample unit: "nanoseconds", "bytes", or "count"/"short"/other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileUnit {
    Nanoseconds,
    Bytes,
    Count,
}

impl ProfileUnit {
    /// Derive the unit from a profile type ID string.
    pub fn from_profile_type_id(id: &str) -> Self {
        match id.split(':').nth(2) {
            Some("nanoseconds") => ProfileUnit::Nanoseconds,
            Some("bytes") => ProfileUnit::Bytes,
            _ => ProfileUnit::Count,
        }
    }

    /// Short label string for use in axis / status line.
    pub fn label(self) -> &'static str {
        match self {
            ProfileUnit::Nanoseconds => "ns",
            ProfileUnit::Bytes => "bytes",
            ProfileUnit::Count => "count",
        }
    }

    /// Format a raw sample value (in base units) as a human-readable string.
    ///
    /// - Nanoseconds: ns / µs / ms / s
    /// - Bytes:       B / KB / MB / GB
    /// - Count:       raw / K / M / G
    pub fn format(self, v: f64) -> String {
        match self {
            ProfileUnit::Nanoseconds => {
                let abs = v.abs();
                if abs >= 1_000_000_000.0 {
                    format!("{:.2}s", v / 1_000_000_000.0)
                } else if abs >= 1_000_000.0 {
                    format!("{:.2}ms", v / 1_000_000.0)
                } else if abs >= 1_000.0 {
                    format!("{:.2}µs", v / 1_000.0)
                } else {
                    format!("{:.0}ns", v)
                }
            }
            ProfileUnit::Bytes => {
                let abs = v.abs();
                if abs >= 1_073_741_824.0 {
                    format!("{:.2}GB", v / 1_073_741_824.0)
                } else if abs >= 1_048_576.0 {
                    format!("{:.2}MB", v / 1_048_576.0)
                } else if abs >= 1_024.0 {
                    format!("{:.2}KB", v / 1_024.0)
                } else {
                    format!("{:.0}B", v)
                }
            }
            ProfileUnit::Count => {
                let abs = v.abs();
                if abs >= 1_000_000_000.0 {
                    format!("{:.2}G", v / 1_000_000_000.0)
                } else if abs >= 1_000_000.0 {
                    format!("{:.2}M", v / 1_000_000.0)
                } else if abs >= 1_000.0 {
                    format!("{:.2}K", v / 1_000.0)
                } else {
                    format!("{:.0}", v)
                }
            }
        }
    }
}
