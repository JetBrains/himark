#[derive(Clone, Copy, PartialEq, PartialOrd, Debug)]
pub struct AnimationClock(f64);

impl AnimationClock {
    pub fn from_millis(now: f64) -> Self {
        Self(now)
    }

    pub fn millis_since(self, earlier: AnimationClock) -> f64 {
        self.0 - earlier.0
    }
}

pub trait AnimVector: Copy {
    fn map(self, f: impl FnMut(f32) -> f32) -> Self;
    fn zip(self, other: Self, f: impl FnMut(f32, f32) -> f32) -> Self;
    fn fold(self, init: f32, f: impl FnMut(f32, f32) -> f32) -> f32;
}

impl<const N: usize> AnimVector for [f32; N] {
    fn map(mut self, mut f: impl FnMut(f32) -> f32) -> Self {
        for component in &mut self {
            *component = f(*component);
        }
        self
    }

    fn zip(mut self, other: Self, mut f: impl FnMut(f32, f32) -> f32) -> Self {
        for (component, other) in self.iter_mut().zip(other) {
            *component = f(*component, other);
        }
        self
    }

    fn fold(self, init: f32, mut f: impl FnMut(f32, f32) -> f32) -> f32 {
        self.into_iter().fold(init, &mut f)
    }
}

impl<A: AnimVector, B: AnimVector> AnimVector for (A, B) {
    fn map(self, mut f: impl FnMut(f32) -> f32) -> Self {
        (self.0.map(&mut f), self.1.map(&mut f))
    }

    fn zip(self, other: Self, mut f: impl FnMut(f32, f32) -> f32) -> Self {
        (self.0.zip(other.0, &mut f), self.1.zip(other.1, &mut f))
    }

    fn fold(self, init: f32, mut f: impl FnMut(f32, f32) -> f32) -> f32 {
        let init = self.0.fold(init, &mut f);
        self.1.fold(init, &mut f)
    }
}

pub trait Animatable: Copy {
    type Vector: AnimVector;
    fn decompose(self) -> Self::Vector;
    fn compose(vector: Self::Vector) -> Self;
}

impl Animatable for f32 {
    type Vector = [f32; 1];

    fn decompose(self) -> [f32; 1] {
        [self]
    }

    fn compose(vector: [f32; 1]) -> Self {
        vector[0]
    }
}

impl Animatable for skia_safe::Size {
    type Vector = [f32; 2];

    fn decompose(self) -> [f32; 2] {
        [self.width, self.height]
    }

    fn compose(vector: [f32; 2]) -> Self {
        skia_safe::Size::new(vector[0], vector[1])
    }
}

impl Animatable for skia_safe::Point {
    type Vector = [f32; 2];

    fn decompose(self) -> [f32; 2] {
        [self.x, self.y]
    }

    fn compose(vector: [f32; 2]) -> Self {
        skia_safe::Point::new(vector[0], vector[1])
    }
}

impl<A: Animatable, B: Animatable> Animatable for (A, B) {
    type Vector = (A::Vector, B::Vector);

    fn decompose(self) -> Self::Vector {
        (self.0.decompose(), self.1.decompose())
    }

    fn compose(vector: Self::Vector) -> Self {
        (A::compose(vector.0), B::compose(vector.1))
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Easing {
    Linear,
    EaseIn,
    EaseOut,
    EaseInOut,
}

impl Easing {
    pub fn apply(self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Easing::Linear => t,
            Easing::EaseIn => t * t * t,
            Easing::EaseOut => {
                let inverse = 1.0 - t;
                1.0 - inverse * inverse * inverse
            }
            Easing::EaseInOut => {
                if t < 0.5 {
                    4.0 * t * t * t
                } else {
                    let inverse = -2.0 * t + 2.0;
                    1.0 - inverse * inverse * inverse / 2.0
                }
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Motion {
    Ease { duration_ms: f64, easing: Easing },
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Animation<T: Animatable> {
    from: T::Vector,
    to: T::Vector,
    motion: Motion,
    progress: f32,
    last_clock: Option<AnimationClock>,
}

impl<T: Animatable> Animation<T> {
    pub fn done(value: T, motion: Motion) -> Self {
        let vector = value.decompose();
        Self {
            from: vector,
            to: vector,
            motion,
            progress: 1.0,
            last_clock: None,
        }
    }

    pub fn set(&mut self, target: T) {
        let target = target.decompose();
        if self.settled() && Self::distance(self.to, target) == 0.0 {
            return;
        }
        self.from = self.current();
        self.to = target;
        self.progress = 0.0;
        self.last_clock = None;
    }

    pub fn jump(&mut self, value: T) {
        let vector = value.decompose();
        self.from = vector;
        self.to = vector;
        self.progress = 1.0;
        self.last_clock = None;
    }

    pub fn advance(&mut self, now: AnimationClock) {
        if self.settled() {
            return;
        }
        let Motion::Ease { duration_ms, .. } = self.motion;
        if let Some(last) = self.last_clock {
            let elapsed = now.millis_since(last).max(0.0);
            self.progress = (self.progress + (elapsed / duration_ms.max(1.0)) as f32).min(1.0);
        }
        self.last_clock = (!self.settled()).then_some(now);
    }

    pub fn running(&self) -> bool {
        !self.settled()
    }

    pub fn value(&self) -> T {
        T::compose(self.current())
    }

    fn settled(&self) -> bool {
        self.progress >= 1.0
    }

    fn current(&self) -> T::Vector {
        let Motion::Ease { easing, .. } = self.motion;
        let t = easing.apply(self.progress);
        self.from.zip(self.to, |from, to| from + (to - from) * t)
    }

    fn distance(a: T::Vector, b: T::Vector) -> f32 {
        a.zip(b, |a, b| (a - b).abs()).fold(0.0, f32::max)
    }
}

#[cfg(test)]
mod tests;
