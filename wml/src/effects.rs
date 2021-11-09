//! Trying out an idea that's been sitting around for a bit.
//! I'll probably want to rip this module out before publishing this library anywhere.

#[derive(Debug, Copy, Clone)]
pub(crate) struct NullHandler;

#[derive(Debug, Copy, Clone)]
pub(crate) struct Printer<'a> {
    data: &'a [u8],
}
impl<'a> Printer<'a> {
    pub(crate) fn new(data: &'a [u8]) -> Self {
        Self { data }
    }
}

pub(crate) struct FallbackHandler<'a, A, B> {
    first: &'a A,
    second: &'a B,
}

pub(crate) enum EffectResult<T> {
    // TODO: decide how to report reason for failure
    Unhandled,
    Handled(T),
}
impl<T> EffectResult<T> {
    #[track_caller]
    pub(crate) fn unwrap(self) -> T {
        match self {
            Self::Unhandled => panic!("unhandled effect"),
            Self::Handled(x) => x,
        }
    }
}

pub(crate) trait Effects {
    fn get_bytes(&self, key: super::StringKey) -> EffectResult<&[u8]>;
    fn get_str(&self, key: super::StringKey) -> EffectResult<&str> {
        match self.get_bytes(key) {
            EffectResult::Handled(bytes) => {
                match ::core::str::from_utf8(bytes) {
                    Ok(s) => EffectResult::Handled(s),
                    Err(_e) => EffectResult::Unhandled,
                }
            },
            EffectResult::Unhandled => EffectResult::Unhandled,
        }
    }
    fn or<'a, H: Effects>(&'a self, other: &'a H) -> FallbackHandler<'_, Self, H> where Self: Sized {
        FallbackHandler {
            first: self,
            second: other,
        }
    }
}

impl<A: Effects, B: Effects> Effects for FallbackHandler<'_, A, B> {
    fn get_bytes(&self, key: super::StringKey) -> EffectResult<&[u8]> {
        match self.first.get_bytes(key) {
            EffectResult::Unhandled => self.second.get_bytes(key),
            x @ EffectResult::Handled(_) => x,
        }
    }
}

impl Effects for NullHandler {
    fn get_bytes(&self, _: super::StringKey) -> EffectResult<&[u8]> {
        EffectResult::Unhandled
    }
}

impl<'a> Effects for Printer<'a> {
    fn get_bytes(&self, key: super::StringKey) -> EffectResult<&[u8]> {
        EffectResult::Handled(&self.data[key.idx .. key.idx + key.len])
    }
}
