//! Reading the game client's interface from a captured frame.
//!
//! Every reader of game UI runs the same stages: find where the thing
//! is ([`window`] for the client's own windows), cut out the text that
//! matters, recognise it, and refuse anything that does not validate.
//! What the client's interface looks like (its frame, its labels, its
//! table headings) is data in the [`profile`], so a redesigned client
//! is answered by editing that file, not this code.

pub mod field;
pub mod geometry;
pub mod locate;
pub mod profile;
pub mod radar;
pub mod seams;
pub mod skills;
pub mod table;
pub mod text;
pub mod validate;
pub mod window;

#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub mod test_support;
