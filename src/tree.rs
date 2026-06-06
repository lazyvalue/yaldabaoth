use tree_sitter::{Parser, Tree};

/// Block boundary info from tree-sitter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockInfo {
    pub start_byte: usize,
    pub end_byte: usize,
    pub start_line: usize,
    pub end_line: usize,
    pub kind: String,
}

pub struct TreeState {
    parser: Parser,
    tree: Option<Tree>,
}

impl TreeState {
    pub fn new() -> Self {
        let mut parser = Parser::new();
        let language = tree_sitter_md::LANGUAGE;
        parser
            .set_language(&language.into())
            .expect("Failed to set tree-sitter markdown language");
        Self { parser, tree: None }
    }

    /// Re-parse `source`. When `edit` is `Some`, reuse the previous tree
    /// incrementally (O(change)) after recording the edit on it; when `None`,
    /// drop the prior tree and do a full parse.
    ///
    /// SAFETY: incremental reuse is sound ONLY if the previous tree was told
    /// about the change via `tree.edit()` with the CORRECT byte+`Point` ranges
    /// first — feeding tree-sitter a stale tree gives it wrong byte offsets,
    /// and on some markdown edits the external scanner's `serialize` then
    /// reads/writes out of bounds → a (heap-nondeterministic) SIGSEGV. The
    /// caller computes `edit` from a single clean splice
    /// (`Document::take_pending_edit`); anything it can't represent exactly
    /// passes `None` (full parse). `incremental_reparse_matches_full_parse`
    /// fuzz-guards that the `Some` path never diverges from a full parse.
    pub fn parse(&mut self, source: &[u8], edit: Option<tree_sitter::InputEdit>) {
        match (edit, self.tree.as_mut()) {
            (Some(e), Some(tree)) => tree.edit(&e),
            // No edit info, or no prior tree → drop it and full-parse.
            _ => self.tree = None,
        }
        self.tree = self.parser.parse(source, self.tree.as_ref());
    }

    pub fn tree(&self) -> Option<&Tree> {
        self.tree.as_ref()
    }

    /// Get top-level block boundaries from the syntax tree.
    pub fn block_boundaries(&self) -> Vec<BlockInfo> {
        let tree = match &self.tree {
            Some(t) => t,
            None => return Vec::new(),
        };

        let root = tree.root_node();
        let mut blocks = Vec::new();

        let mut cursor = root.walk();
        for child in root.children(&mut cursor) {
            if child.kind() == "\n" || child.is_extra() {
                continue;
            }
            // The MDeiml markdown grammar wraps blocks in "section" nodes.
            // Descend into sections to get the actual block-level elements.
            if child.kind() == "section" {
                let mut section_cursor = child.walk();
                for section_child in child.children(&mut section_cursor) {
                    if section_child.kind() == "\n" || section_child.is_extra() {
                        continue;
                    }
                    blocks.push(BlockInfo {
                        start_byte: section_child.start_byte(),
                        end_byte: section_child.end_byte(),
                        start_line: section_child.start_position().row,
                        end_line: section_child.end_position().row,
                        kind: section_child.kind().to_string(),
                    });
                }
            } else {
                blocks.push(BlockInfo {
                    start_byte: child.start_byte(),
                    end_byte: child.end_byte(),
                    start_line: child.start_position().row,
                    end_line: child.end_position().row,
                    kind: child.kind().to_string(),
                });
            }
        }

        blocks
    }

    /// Find which block index contains the given byte offset.
    pub fn active_block_at_byte(&self, byte_offset: usize) -> Option<usize> {
        let blocks = self.block_boundaries();
        for (i, block) in blocks.iter().enumerate() {
            if byte_offset >= block.start_byte && byte_offset < block.end_byte {
                return Some(i);
            }
        }
        // If past last block, return last block
        if !blocks.is_empty() {
            Some(blocks.len() - 1)
        } else {
            None
        }
    }

    /// Get changed byte ranges between the old and new tree.
    /// Call after edit() + parse() to find what blocks need re-rendering.
    pub fn changed_ranges(&self, old_tree: &Tree) -> Vec<std::ops::Range<usize>> {
        match &self.tree {
            Some(new_tree) => {
                let ranges = old_tree.changed_ranges(new_tree);
                ranges.map(|r| r.start_byte..r.end_byte).collect()
            }
            None => Vec::new(),
        }
    }
}

impl Default for TreeState {
    fn default() -> Self {
        Self::new()
    }
}
