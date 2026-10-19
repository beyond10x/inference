//! The `inference.stream.Generation` vocabulary of `ess/domains/stream.yaml`: one output epoch of
//! a producing node within a run.
//!
//! A [`Generation`] is started by [`Generation::start`] and is [`GenerationState::Active`] until
//! it completes, is superseded or is cancelled; all three are terminal. On interruption the
//! current epoch is superseded, and output from a superseded or cancelled generation is stale.
//! Each transition is a method that returns the moved generation and the event its command emits,
//! or [`GenerationStateConflict`] carrying the state the generation holds; a refused move changes
//! nothing. The segments a generation owns are `inference-text`'s; discarding them when the
//! generation is superseded or cancelled is the store's, which holds both. Serialized names are
//! the ESS names.

use std::error::Error;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::id::{GenerationId, NodeId, RunId};

/// `inference.stream.Generation.State`: where a generation stands. Serialized as written in ESS
/// (`Active`, `Superseded`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum GenerationState {
    /// The epoch is current and admits new output.
    Active,
    /// The generation produced its last output.
    Completed,
    /// An interruption marked the epoch stale; its results are rejected at irreversible sinks.
    Superseded,
    /// Delivery from the generation was cancelled; computation may still finish.
    Cancelled,
}

impl GenerationState {
    /// Every state, in the order `ess/domains/stream.yaml` lists them.
    pub const ALL: [Self; 4] = [
        Self::Active,
        Self::Completed,
        Self::Superseded,
        Self::Cancelled,
    ];
}

/// `inference.stream.StartGeneration`: the input that starts a [`Generation`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartGeneration {
    pub run_id: RunId,
    pub node_id: NodeId,
    pub epoch: i64,
}

/// `inference.stream.GenerationStarted`: a new output epoch is active for the node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenerationStarted {
    pub generation_id: GenerationId,
    pub run_id: RunId,
    pub epoch: i64,
}

/// `inference.stream.GenerationCompleted`: the generation produced its last output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenerationCompleted {
    pub generation_id: GenerationId,
}

/// `inference.stream.GenerationSuperseded`: the epoch is stale; its results are rejected at
/// irreversible sinks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenerationSuperseded {
    pub generation_id: GenerationId,
}

/// `inference.stream.GenerationCancelled`: delivery from the generation is cancelled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenerationCancelled {
    pub generation_id: GenerationId,
}

/// `inference.stream.GenerationStateConflict`: the generation is not Active, so nothing moved.
/// `current` is the state it holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenerationStateConflict {
    pub current: GenerationState,
}

impl fmt::Display for GenerationStateConflict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "the generation is {:?}, not Active, so nothing moved",
            self.current
        )
    }
}

impl Error for GenerationStateConflict {}

/// `inference.stream.GenerationNotFound`: no generation carries the identity a command named.
/// `generation_id` is the identity that was sent. Only a store holding the generations can decide
/// it; no method of [`Generation`] returns it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenerationNotFound {
    pub generation_id: GenerationId,
}

impl fmt::Display for GenerationNotFound {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "no generation carries the identity {}",
            self.generation_id
        )
    }
}

impl Error for GenerationNotFound {}

/// `inference.stream.Generation`: one output epoch of a node within a run. It references its run
/// by `run_id` and its producing node by `node_id`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Generation {
    generation_id: GenerationId,
    run_id: RunId,
    node_id: NodeId,
    epoch: i64,
    state: GenerationState,
}

impl Generation {
    /// Runs the `started` outcome of `inference.stream.StartGeneration`: generates a
    /// [`GenerationId`] and returns the Active generation and its [`GenerationStarted`].
    ///
    /// The refusals of the command, a run no store holds, a run that is not Active and a node no
    /// pipeline declares, are decided by a store holding the runs and nodes before it calls this.
    #[must_use]
    pub fn start(input: StartGeneration) -> (Self, GenerationStarted) {
        Self::start_as(GenerationId::generate(), input)
    }

    /// Runs the `started` outcome of `inference.stream.StartGeneration` under `generation_id`
    /// instead of a generated one, and otherwise as [`Generation::start`]. It is for a store that
    /// assigns the identities itself, such as the counter of `inference-testkit`, whose runs must
    /// report the same identities each time.
    #[must_use]
    pub fn start_as(
        generation_id: GenerationId,
        input: StartGeneration,
    ) -> (Self, GenerationStarted) {
        let event = GenerationStarted {
            generation_id,
            run_id: input.run_id,
            epoch: input.epoch,
        };
        let generation = Self {
            generation_id,
            run_id: input.run_id,
            node_id: input.node_id,
            epoch: input.epoch,
            state: GenerationState::Active,
        };
        (generation, event)
    }

    /// The generation moved to `to`, if it is Active; a terminal generation refuses every move.
    fn moved(&self, to: GenerationState) -> Result<Self, GenerationStateConflict> {
        if self.state != GenerationState::Active {
            return Err(GenerationStateConflict {
                current: self.state,
            });
        }
        Ok(Self {
            state: to,
            ..self.clone()
        })
    }

    /// Runs the `completed` outcome of `inference.stream.CompleteGeneration`: the generation
    /// moves from Active to Completed.
    ///
    /// # Errors
    ///
    /// [`GenerationStateConflict`] carrying the held state when the generation is not Active.
    pub fn complete(&self) -> Result<(Self, GenerationCompleted), GenerationStateConflict> {
        let generation = self.moved(GenerationState::Completed)?;
        let event = GenerationCompleted {
            generation_id: self.generation_id,
        };
        Ok((generation, event))
    }

    /// Runs the `superseded` outcome of `inference.stream.SupersedeGeneration`: the generation
    /// moves from Active to Superseded. Its unplayed segments are discarded by the store in the
    /// same outcome.
    ///
    /// # Errors
    ///
    /// [`GenerationStateConflict`] carrying the held state when the generation is not Active.
    pub fn supersede(&self) -> Result<(Self, GenerationSuperseded), GenerationStateConflict> {
        let generation = self.moved(GenerationState::Superseded)?;
        let event = GenerationSuperseded {
            generation_id: self.generation_id,
        };
        Ok((generation, event))
    }

    /// Runs the `cancelled` outcome of `inference.stream.CancelGeneration`: the generation moves
    /// from Active to Cancelled. Its unplayed segments are discarded by the store in the same
    /// outcome.
    ///
    /// # Errors
    ///
    /// [`GenerationStateConflict`] carrying the held state when the generation is not Active.
    pub fn cancel(&self) -> Result<(Self, GenerationCancelled), GenerationStateConflict> {
        let generation = self.moved(GenerationState::Cancelled)?;
        let event = GenerationCancelled {
            generation_id: self.generation_id,
        };
        Ok((generation, event))
    }

    #[must_use]
    pub const fn generation_id(&self) -> GenerationId {
        self.generation_id
    }

    #[must_use]
    pub const fn run_id(&self) -> RunId {
        self.run_id
    }

    #[must_use]
    pub const fn node_id(&self) -> NodeId {
        self.node_id
    }

    #[must_use]
    pub const fn epoch(&self) -> i64 {
        self.epoch
    }

    #[must_use]
    pub const fn state(&self) -> GenerationState {
        self.state
    }
}
