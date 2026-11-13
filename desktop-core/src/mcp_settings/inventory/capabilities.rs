pub(super) fn capabilities(
    supported: bool,
    detected: bool,
    selected: bool,
    present: bool,
    configured: bool,
    failed: bool,
) -> (bool, bool, bool) {
    (
        supported && detected && !configured,
        supported && detected && (selected || present),
        present || (supported && selected) || failed,
    )
}
