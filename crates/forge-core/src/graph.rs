//! Laying out a commit graph: which lane each commit sits in and how the lines between rows run,
//! as git's own `--graph` does, so a view can draw it.

use crate::git::GraphCommit;

/// One row of the graph. Lanes are columns; a `color` is a small number that stays with a lane's line
/// from where it starts to where it ends, so lines keep their colour down the page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphRow {
    /// The lane this commit's dot sits in.
    pub node: usize,
    pub node_color: usize,
    /// Lines entering the row from above, per lane.
    pub top: Vec<Option<usize>>,
    /// Lines leaving the row downward, per lane.
    pub bottom: Vec<Option<usize>>,
    /// Lanes that end at this commit (a merge's other parents' lines, seen from below).
    pub converge: Vec<usize>,
    /// Lanes this commit's extra parents continue in.
    pub diverge: Vec<usize>,
}

impl GraphRow {
    /// How many lane columns this row needs.
    pub fn width(&self) -> usize {
        self.top.len().max(self.bottom.len()).max(self.node + 1)
    }
}

/// A free lane (reusing a gap on the left before adding a column).
fn free_lane(lanes: &mut Vec<Option<(String, usize)>>) -> usize {
    match lanes.iter().position(Option::is_none) {
        Some(i) => i,
        None => {
            lanes.push(None);
            lanes.len() - 1
        }
    }
}

pub fn layout(commits: &[GraphCommit]) -> Vec<GraphRow> {
    let mut lanes: Vec<Option<(String, usize)>> = Vec::new();
    let mut next_color = 0usize;
    let mut rows = Vec::with_capacity(commits.len());
    for commit in commits {
        let snapshot = |lanes: &Vec<Option<(String, usize)>>| -> Vec<Option<usize>> { lanes.iter().map(|l| l.as_ref().map(|(_, c)| *c)).collect() };
        let top = snapshot(&lanes);
        let waiting: Vec<usize> = (0..lanes.len()).filter(|&i| lanes[i].as_ref().is_some_and(|(h, _)| *h == commit.hash)).collect();
        // A commit nobody expects starts a new line (a branch tip).
        let (node, node_color) = match waiting.first() {
            Some(&i) => (i, lanes[i].as_ref().map_or(0, |(_, c)| *c)),
            None => {
                let i = free_lane(&mut lanes);
                next_color += 1;
                (i, next_color)
            }
        };
        let converge: Vec<usize> = waiting.iter().skip(1).copied().collect();
        for &i in &waiting {
            lanes[i] = None;
        }
        let mut diverge = Vec::new();
        for (n, parent) in commit.parents.iter().enumerate() {
            let existing = lanes.iter().position(|l| l.as_ref().is_some_and(|(h, _)| h == parent));
            match (n, existing) {
                // The first parent carries on in this commit's lane, even when another lane also waits for it:
                // the two lines then join at that parent's row, as in `git log --graph`.
                (0, _) => lanes[node] = Some((parent.clone(), node_color)),
                // A later parent already waited for elsewhere: draw a line to that lane.
                (_, Some(j)) => diverge.push(j),
                (_, None) => {
                    let k = free_lane(&mut lanes);
                    next_color += 1;
                    lanes[k] = Some((parent.clone(), next_color));
                    diverge.push(k);
                }
            }
        }
        while lanes.last().is_some_and(Option::is_none) {
            lanes.pop();
        }
        rows.push(GraphRow { node, node_color, top, bottom: snapshot(&lanes), converge, diverge });
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commit(hash: &str, parents: &[&str]) -> GraphCommit {
        GraphCommit { hash: hash.to_owned(), parents: parents.iter().map(|p| (*p).to_owned()).collect(), author: String::new(), when: String::new(), refs: Vec::new(), subject: String::new() }
    }

    #[test]
    fn a_straight_history_stays_in_one_lane() {
        let rows = layout(&[commit("c", &["b"]), commit("b", &["a"]), commit("a", &[])]);
        assert!(rows.iter().all(|r| r.node == 0 && r.width() == 1));
        assert_eq!(rows[0].top, Vec::<Option<usize>>::new(), "the tip has nothing above it");
        assert_eq!(rows[1].top, rows[0].bottom, "a line runs from one row into the next");
        assert_eq!(rows[2].bottom, Vec::<Option<usize>>::new(), "the root ends the line");
    }

    #[test]
    fn a_merge_opens_a_lane_for_its_second_parent_and_it_joins_back_at_the_fork() {
        // m merges b (first parent, main) and f (feature); both come from a.
        let rows = layout(&[commit("m", &["b", "f"]), commit("f", &["a"]), commit("b", &["a"]), commit("a", &[])]);
        assert_eq!(rows[0].diverge, [1], "the merge's second parent gets lane 1");
        assert_eq!(rows[1].node, 1, "the feature commit sits in that lane");
        assert_eq!(rows[2].node, 0, "main continues in lane 0");
        // At the fork point both lanes are waiting for a: lane 0 holds the commit, lane 1 joins it.
        assert_eq!((rows[3].node, rows[3].converge.clone()), (0, vec![1]));
        assert_ne!(rows[1].node_color, rows[2].node_color, "the two lines have different colours");
    }

    #[test]
    fn branch_tips_get_their_own_lanes_and_join_at_the_shared_ancestor() {
        // Two tips x and y share the ancestor a; a later tip z of r starts a lane of its own again.
        let rows = layout(&[commit("x", &["a"]), commit("y", &["a"]), commit("a", &["r"]), commit("z", &["r"]), commit("r", &[])]);
        assert_eq!((rows[0].node, rows[1].node), (0, 1));
        assert_eq!(rows[2].converge, [1], "y's line ends at a");
        assert_eq!(rows[3].node, 1, "z is a new tip, so it opens a new lane");
        assert_eq!((rows[4].node, rows[4].converge.clone()), (0, vec![1]));
    }
}
