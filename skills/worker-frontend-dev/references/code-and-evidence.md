# Code and evidence discovery

Apply this procedure for structural repository discovery and when saved report
detail is needed.

If the assignment brief names `Known locations`, treat them as focused
references from coordinator orientation, not as proof that the source is current.
Read those current sources directly before editing or drawing conclusions. Start
documentation-index reading and broad discovery only when the named locations are
insufficient or the change touches a contract or documentation surface.

When an external code-index MCP server (such as `codebase-memory-mcp`) is attached,
use its own tools first for every code discovery question: definitions, callers, dependencies,
impact and implementation reads. Use repository-native search or file reads only
for literal text, configuration and non-code files, for paths the index does not
cover, or when no matching healthy index exists. Discover the needed operation
names and read their complete advertised schemas.

Call `list_projects` once and select the project whose full root path matches the
exact workspace, not a similar project or another worktree; never guess a project
name from the path. If multiple entries match, compare `index_status` and relevant
`check_index_coverage` results to select the healthy index covering the assigned
scope. A ready index can still exclude the entire subsystem. If no matching index
exists and `index_repository` is among your granted tools, make one initial attempt
for the authorized exact workspace root; otherwise record the missing index as a
limitation. Do not substitute the logical repository root for a linked worktree,
change ignore rules, or retry argument variations after an indexing error.

An indexer can reject an otherwise valid linked worktree. Treat that failed single
attempt as unavailable for the exact workspace, record the returned error, and use
bounded repository-native reads for the concrete question. A readable linked
worktree can still be rejected by an indexer; do not assume another worktree's
successful index covers it. Follow the live schema for tool arguments: this guidance
only determines when an index attempt is appropriate.

Use the smallest operation that answers the question: `search_graph` for definitions,
`trace_path` for callers, dependencies and impact paths, and `get_code_snippet` for the
selected implementation. Use scoped `get_architecture` for orientation, or
`get_graph_schema` before a complex `query_graph`; `detect_changes` serves an
established Git comparison. Literal/configuration/documentation text may use
`search_code` or bounded native search. Follow live schemas for arguments.

Confirm consequential graph findings against current source. Check coverage for cited or
edited paths and scopes behind negative or exhaustive claims. Follow returned
pagination when the question requires more results, and account for omitted tests.
Empty results and clean best-effort coverage do not prove completeness. Stale,
skipped, excluded or unsupported files require bounded direct inspection.

If no matching healthy index is available, record the limitation and use one bounded
repository-native fallback for the concrete question. Do not loop on unavailable
graph tools, infer absence from no hits or rebuild an index after every small change.
Fresh source, executable configuration and observed tests outrank derived graph
relationships.

In code-investigation reports, identify the selected index and matching project,
or the concrete limitation that required source fallback. Merely listing available
tools is not evidence that graph discovery was performed.

Treat report references as opaque values supplied by the coordinator or a live
catalogue result. Read them through Cortex MCP `read_report`, not the code-index
server or a native file tool. Read only reports relevant to the assignment. Begin each document
at no more than 4,000 characters and continue with its cursor until the needed fact
is found. Reuse unchanged pages. Record the source revision, artifact revision and
remaining coverage gaps in the published evidence.
