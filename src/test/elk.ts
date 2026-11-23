// jsdom has no Worker: tests lay graphs out with ELK's main-thread bundle instead.
// Import it from a test file that lays out a flow graph.

import ELK from "elkjs/lib/elk.bundled.js";

import { setLayoutEngine } from "@/features/review/flow/flow-layout";

setLayoutEngine(new ELK());
