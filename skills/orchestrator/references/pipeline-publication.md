# Runtime pipeline

The runtime publishes the single newest-first pipeline from durable state.
The coordinator contributes plans and synthesis through the advertised proposal
API. Do not write a pipeline edition or a task-completion decision yourself.
Explain the factual basis of your synthesis and distinguish implementation,
observed checks and delivery. Only the durable runtime state establishes completion.
