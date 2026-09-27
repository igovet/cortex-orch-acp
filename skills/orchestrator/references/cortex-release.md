# Cortex release work

Read the target checkout's current ACP documentation and release requirements.
Keep source checks and installed-product observations distinct. Use the required
semantic version and regenerate the complete payload hash after payload changes.
Never represent an unrun real-client observation as passing. Paid live checks
run only on an explicit owner request naming `cli` or `desktop`, and only through
the isolated dev Paseo setup described in the live-qualification reference, using
the required probe flags and a reference naming that request; otherwise report
them as not run. Keep the default automated suite no-paid.