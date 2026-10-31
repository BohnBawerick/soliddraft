//! Measure commands owned by V2.

use crate::commands::{Agents, CommandSpec, Undo};
use crate::icons::Icon;

pub fn commands() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("measure_entity", "Measure entity", Icon::Measure, "Prove one edge length, face area or straight-edge midpoint")
            .agents(Agents::Tool, "Returns exact or bounded evidence on the current geometry without changing the part. quantity midpoint gives a straight edge's proven midpoint, one enclosure per coordinate.", super::tools::measure_params),
        CommandSpec::new("track", "Track this", Icon::Measure, "Track one proven edge length, face area or pair scalar")
            .agents(Agents::Mcp, "Stores a persistent reference with an optional check. With entity_b it tracks a pair scalar, as the measure tool proves it. A standing measurement request follows edits or becomes stale. Tracked values cannot drive geometry.", super::tools::track_params)
            .undo(Undo::Step),
    ]
}
