// Navigation shapes: the Sessions tree, ⌘K search, workspace activity, and artifact outlines.

import type { Artifact } from "./gen/Artifact";
import type { Heading } from "./gen/Heading";
import type { SearchKind } from "./gen/SearchKind";

export type { OpenGateRef } from "./gen/OpenGateRef";
export type { RunningExecution } from "./gen/RunningExecution";
export type { SearchHit } from "./gen/SearchHit";
export type { SearchResults } from "./gen/SearchResults";
export type { TreeGroup } from "./gen/TreeGroup";
export type { TreeRun } from "./gen/TreeRun";
export type { TreeSession } from "./gen/TreeSession";
export type { WorkspaceActivity } from "./gen/WorkspaceActivity";
export type { WorkspaceTree } from "./gen/WorkspaceTree";

export type SearchHitKind = SearchKind;
export type ArtifactHeading = Heading;
export type ArtifactWithHeadings = Artifact;
