// The hard set is sound; see soundness.ts. Its own file, so it runs in
// parallel with the easy set.
import { soundness } from "./soundness.ts";

soundness("hard", 6);
