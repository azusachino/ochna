# Dive: Call-Graph PageRank Centrality and Tarjan SCC Cycle Analysis

This document explains how `scripts/report.py` combines static call-graph analysis from **Cornell CS 6120 (Advanced Compilers)** and spectral/walk-based ranking from **Stanford CS224W (Machine Learning with Graphs)** directly over `ochna`'s SQLite relational index (`.ochna/ochna.db`).

---

## 1. In-degree hotspots vs. PageRank centrality (`CS224W`)

A naive in-degree query (`COUNT(*) GROUP BY target_nid`) ranks functions strictly by how many call sites mention them directly. However, in layered systems codebases (such as `clones/tokio`, `clones/netty`, or `clones/kubernetes`):

- Utility helpers (logging, error wrappers) often have high raw in-degree from leaf call sites.
- Core architectural bottlenecks (such as a scheduler tick or state-machine transition) may be called by only 2 or 3 high-importance dispatchers, which themselves sit at the center of hundreds of callers.

**Call-Graph PageRank** (`compute_callgraph_pagerank` in `scripts/report.py`) models a random walker following `calls` edges from caller to callee with damping factor $\alpha = 0.85$ and uniform teleportation across non-test symbols ($V_{\text{prod}}$):

$$r^{(t+1)}(v) = \frac{1 - \alpha}{|V_{\text{prod}}|} + \alpha \sum_{u \in \text{callers}(v)} \frac{r^{(t)}(u)}{\text{outdeg}(u)} + \frac{\alpha}{|V_{\text{prod}}|} \sum_{w : \text{outdeg}(w) = 0} r^{(t)}(w)$$

Because importance flows transitively along directed call chains, PageRank surfaces both direct fan-in hubs and the deeper foundational primitives those hubs delegate to.

---

## 2. Tarjan's Strongly Connected Components (`CS 6120`)

In interprocedural compiler optimization (inlining heuristics, stack-depth bounding, and bottom-up summary dataflow analysis), a call graph is a **Directed Acyclic Graph (DAG)** if and only if the program contains no direct or mutual recursion.

`find_recursive_sccs` in `scripts/report.py` runs **Tarjan's $\mathcal{O}(|V| + |E|)$ Strongly Connected Components (SCC)** algorithm over `edges WHERE kind = 'calls'`:

1. Each node $v$ receives a discovery timestamp `indices[v]` and a lowest reachable ancestor index `lowlink[v]` on the active DFS stack.
2. Whenever `lowlink[v] == indices[v]`, popping the stack up to $v$ yields one complete strongly connected component $C$.
3. Any component with $|C| > 1$ (mutual recursion, e.g. recursive-descent parser functions `parse_expr <-> parse_term`) or $|C| = 1$ with a self-edge $(v, v) \in E$ (direct recursion) is reported as a call-graph cycle.
4. Collapsing each SCC into a supernode yields the **condensation DAG**, enabling topological bottom-up interprocedural analysis.
