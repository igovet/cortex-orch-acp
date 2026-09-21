# Interactive resources

Read this reference only when the assignment requires a browser, device, emulator,
port, server or external application session.

Use only a resource created for or explicitly assigned to this worker. Never inspect
global processes, ports, tabs or devices to discover another worker's resource. A
surface owned by another assignment is unavailable unless the coordinator
establishes isolation or transfers ownership.

Use the live tool state and receipts to select, create or reuse only owned resources.
Before depending on a service or application, observe that its required state is
ready. After an interaction, inspect enough current state to determine which effects
occurred before making a dependent claim or action. Use stable accessible controls
when available and do not reuse snapshot-local handles after their observed state
has changed. Call grouping and tab count follow the tool's guarantees and the task,
not a fixed choreography.

When an operation returns a running handle, retain and poll or resume that same
handle to a terminal receipt. Never launch the same or an equivalent command or
operation while the original handle remains active. Keep waits bounded. If the
operation cannot reach a terminal result within the assignment constraints, cancel
or close only the owned handle, retain the timeout or cancellation receipt, and
report the check unverified.

Stop or close only sessions created by this assignment. Before report publication,
obtain a terminal receipt for every command session and leave no owned watcher,
server or helper running. If the required surface is unavailable, report the exact
limitation instead of probing unrelated browsers, ports or applications.
