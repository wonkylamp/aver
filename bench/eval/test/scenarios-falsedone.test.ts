// The falsedone set is sound (in it, every task has a naive fake whose
// tests are green and whose hidden tests fail); see soundness.ts.
import { soundness } from "./soundness.ts";

soundness("falsedone", 4);
