//! Backend-independent simulation results.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VecKind {
    Time,
    Frequency,
    Voltage,
    Current,
    Sweep,
    Other,
}

impl VecKind {
    pub fn unit(self) -> &'static str {
        match self {
            VecKind::Time => "s",
            VecKind::Frequency => "Hz",
            VecKind::Voltage => "V",
            VecKind::Current => "A",
            VecKind::Sweep | VecKind::Other => "",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Vector {
    pub name: String,
    pub kind: VecKind,
    pub re: Vec<f64>,
    /// Imaginary part for complex (AC) data.
    pub im: Option<Vec<f64>>,
}

impl Vector {
    pub fn real(name: impl Into<String>, kind: VecKind, re: Vec<f64>) -> Self {
        Self { name: name.into(), kind, re, im: None }
    }

    pub fn mag(&self) -> Vec<f64> {
        match &self.im {
            Some(im) => self.re.iter().zip(im).map(|(r, i)| (r * r + i * i).sqrt()).collect(),
            None => self.re.iter().map(|r| r.abs()).collect(),
        }
    }

    pub fn db(&self) -> Vec<f64> {
        self.mag().into_iter().map(|m| 20.0 * m.max(1e-300).log10()).collect()
    }

    pub fn phase_deg(&self) -> Vec<f64> {
        match &self.im {
            Some(im) => self.re.iter().zip(im).map(|(r, i)| i.atan2(*r).to_degrees()).collect(),
            None => self.re.iter().map(|r| if *r < 0.0 { 180.0 } else { 0.0 }).collect(),
        }
    }
}

/// Operating point information for one device (built-in engine only).
#[derive(Debug, Clone)]
pub struct DeviceOp {
    pub name: String,
    pub region: Option<String>,
    pub values: Vec<(String, f64)>,
}

#[derive(Debug, Clone)]
pub struct Dataset {
    /// "op", "dc", "ac" or "tran".
    pub analysis: String,
    pub title: String,
    /// Index into `vectors` of the independent variable (None for op).
    pub scale: Option<usize>,
    pub vectors: Vec<Vector>,
    /// Device operating points (OP analysis).
    pub devices: Vec<DeviceOp>,
}

impl Dataset {
    pub fn scale_vector(&self) -> Option<&Vector> {
        self.scale.map(|i| &self.vectors[i])
    }

    pub fn get(&self, name: &str) -> Option<&Vector> {
        self.vectors.iter().find(|v| v.name.eq_ignore_ascii_case(name))
    }

    pub fn signals(&self) -> impl Iterator<Item = &Vector> {
        self.vectors.iter().enumerate().filter(move |(i, _)| Some(*i) != self.scale).map(|(_, v)| v)
    }

    pub fn len(&self) -> usize {
        self.vectors.first().map(|v| v.re.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[derive(Debug, Clone, Default)]
pub struct SimResults {
    pub backend: String,
    pub datasets: Vec<Dataset>,
    pub log: String,
    pub elapsed_ms: f64,
}

impl SimResults {
    pub fn dataset(&self, analysis: &str) -> Option<&Dataset> {
        self.datasets.iter().find(|d| d.analysis == analysis)
    }
}
