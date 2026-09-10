use skia_safe::Size;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Constraints {
    pub min: Size,
    pub max: Size,
}

impl Constraints {
    pub fn tight(size: Size) -> Self {
        Self {
            min: size,
            max: size,
        }
    }

    pub fn loosen(self) -> Self {
        Self {
            min: Size::default(),
            max: self.max,
        }
    }
}
