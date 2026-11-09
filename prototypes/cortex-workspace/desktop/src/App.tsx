import { lazy, Suspense } from "solid-js";

const Workspace = lazy(() => import("./workspace/ui"));

/** The approved Workspace is the whole app. Earlier explorations are no longer routed; an old address lands on Not found. */
export default function App() {
  return <Suspense><Workspace /></Suspense>;
}
