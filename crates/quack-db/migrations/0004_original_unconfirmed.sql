-- An undo whose original's change was never confirmed and that Discord still
-- doesn't show after the undo's round: the original may yet land, so the
-- undo fails where staff see it instead of finishing `no_change` (design
-- §5.6, implementation notes).
ALTER TYPE action_failure ADD VALUE 'original_unconfirmed';
