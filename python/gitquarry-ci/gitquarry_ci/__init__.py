"""Python SDK for `gitquarry-ci` pipeline definitions. Never executes
anything -- only builds a `JobGraph` and dumps it as JSON to stdout via
`GraphBuilder.dump()`. `gitquarry-ci` is the sole executor of the resulting
graph, regardless of which SDK/language produced it.
"""

from gitquarry_ci.context import PipelineContext, context
from gitquarry_ci.graph import GraphBuilder, graph
from gitquarry_ci.job import JobBuilder, job
from gitquarry_ci.validate import GraphError
from gitquarry_ci.wire_generated import JobKind

__all__ = [
    "job",
    "JobBuilder",
    "graph",
    "GraphBuilder",
    "context",
    "PipelineContext",
    "JobKind",
    "GraphError",
]
