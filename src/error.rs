use crate::player::PlaybackError;
use std::error::Error;
use std::fmt;
use std::io;

#[derive(Debug)]
pub struct AfqueueError {
    source: Box<dyn Error>,
    context: Vec<ErrorCtx>,
}

impl AfqueueError {
    fn new(source: Box<dyn Error>) -> Self {
        AfqueueError {
            source,
            context: Vec::new(),
        }
    }

    fn add_context(&mut self, ctx: ErrorCtx) {
        self.context.push(ctx);
    }
}

impl fmt::Display for AfqueueError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let err = &self.source;
        write!(f, "Oh no, ran into error ")?;
        for ctx in &self.context {
            write!(f, "while {ctx}, ")?;
        }
        write!(f, "{err}")?;
        Ok(())
    }
}

impl Error for AfqueueError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.source.as_ref())
    }
}

//TODO: Do we actually want to box the error?
impl From<io::Error> for AfqueueError {
    fn from(err: io::Error) -> AfqueueError {
        AfqueueError::new(Box::new(err))
    }
}

impl From<PlaybackError> for AfqueueError {
    fn from(err: PlaybackError) -> AfqueueError {
        AfqueueError::new(Box::new(err))
    }
}

#[derive(Debug)]
pub enum ErrorCtx {
    PlayingBack(String),
}

impl fmt::Display for ErrorCtx {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            ErrorCtx::PlayingBack(filepath) => write!(f, "playing back file '{filepath}'"),
        }
    }
}

pub trait ErrorContext<T> {
    fn with(self, context: ErrorCtx) -> Result<T, AfqueueError>;
}

impl<T> ErrorContext<T> for Result<T, AfqueueError> {
    fn with(mut self, context: ErrorCtx) -> Result<T, AfqueueError> {
        if let Err(ref mut err) = self {
            err.add_context(context);
        }

        self
    }
}
