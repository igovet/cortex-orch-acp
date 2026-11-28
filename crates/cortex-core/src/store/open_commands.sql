CREATE INDEX IF NOT EXISTS runtime_command_outcomes_ref ON runtime_events(
 task_id, coalesce(json_extract(payload_json,'$.receipt.tool_call_ref'),json_extract(payload_json,'$.ref')), id
) WHERE type IN ('NATIVE_COMMAND_FINISHED','NATIVE_COMMAND_RECOVERED');

CREATE TABLE IF NOT EXISTS runtime_open_commands (
 task_id TEXT NOT NULL REFERENCES runtime_tasks(task_id) ON DELETE CASCADE,
 start_id INTEGER NOT NULL REFERENCES runtime_events(id) ON UPDATE CASCADE ON DELETE CASCADE,
 command_ref,
 PRIMARY KEY(task_id,start_id)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS runtime_open_commands_ref ON runtime_open_commands(task_id,command_ref,start_id);
CREATE INDEX IF NOT EXISTS runtime_open_commands_event ON runtime_open_commands(start_id);

CREATE TABLE IF NOT EXISTS runtime_open_commands_dirty (
 task_id TEXT PRIMARY KEY REFERENCES runtime_tasks(task_id) ON DELETE CASCADE
) WITHOUT ROWID;

CREATE TRIGGER IF NOT EXISTS runtime_open_commands_start
AFTER INSERT ON runtime_events WHEN NEW.type='NATIVE_COMMAND_STARTED'
BEGIN
 INSERT INTO runtime_open_commands(task_id,start_id,command_ref)
 SELECT NEW.task_id,NEW.id,json_extract(NEW.payload_json,'$.ref')
 WHERE NOT EXISTS(
  SELECT 1 FROM runtime_events f WHERE f.task_id=NEW.task_id AND f.id>NEW.id
   AND f.type IN ('NATIVE_COMMAND_FINISHED','NATIVE_COMMAND_RECOVERED')
   AND coalesce(json_extract(f.payload_json,'$.receipt.tool_call_ref'),json_extract(f.payload_json,'$.ref'))=json_extract(NEW.payload_json,'$.ref')
 );
END;

CREATE TRIGGER IF NOT EXISTS runtime_open_commands_settle
AFTER INSERT ON runtime_events WHEN NEW.type IN ('NATIVE_COMMAND_FINISHED','NATIVE_COMMAND_RECOVERED')
BEGIN
 DELETE FROM runtime_open_commands WHERE task_id=NEW.task_id AND start_id<NEW.id
  AND command_ref=coalesce(json_extract(NEW.payload_json,'$.receipt.tool_call_ref'),json_extract(NEW.payload_json,'$.ref'));
END;

-- UPDATE and DELETE are unusual for this journal. Mark affected tasks before a
-- reader can use their old projection. Initialization rebuilds those tasks.
CREATE TRIGGER IF NOT EXISTS runtime_open_commands_change
AFTER UPDATE ON runtime_events
WHEN (OLD.type IN ('NATIVE_COMMAND_STARTED','NATIVE_COMMAND_FINISHED','NATIVE_COMMAND_RECOVERED')
   OR NEW.type IN ('NATIVE_COMMAND_STARTED','NATIVE_COMMAND_FINISHED','NATIVE_COMMAND_RECOVERED'))
 AND (OLD.task_id IS NOT NEW.task_id OR OLD.id IS NOT NEW.id OR OLD.type IS NOT NEW.type
  OR (CASE WHEN OLD.type='NATIVE_COMMAND_STARTED' THEN json_extract(OLD.payload_json,'$.ref')
      ELSE coalesce(json_extract(OLD.payload_json,'$.receipt.tool_call_ref'),json_extract(OLD.payload_json,'$.ref')) END)
   IS NOT (CASE WHEN NEW.type='NATIVE_COMMAND_STARTED' THEN json_extract(NEW.payload_json,'$.ref')
      ELSE coalesce(json_extract(NEW.payload_json,'$.receipt.tool_call_ref'),json_extract(NEW.payload_json,'$.ref')) END))
BEGIN
 INSERT INTO runtime_open_commands_dirty VALUES(OLD.task_id) ON CONFLICT(task_id) DO NOTHING;
 INSERT INTO runtime_open_commands_dirty VALUES(NEW.task_id) ON CONFLICT(task_id) DO NOTHING;
END;

CREATE TRIGGER IF NOT EXISTS runtime_open_commands_remove
AFTER DELETE ON runtime_events WHEN OLD.type IN ('NATIVE_COMMAND_STARTED','NATIVE_COMMAND_FINISHED','NATIVE_COMMAND_RECOVERED')
BEGIN
 DELETE FROM runtime_open_commands WHERE start_id=OLD.id;
 INSERT INTO runtime_open_commands_dirty VALUES(OLD.task_id) ON CONFLICT(task_id) DO NOTHING;
END;

-- SQLite can replace a conflicting row without invoking its DELETE trigger.
-- Invalidate the original task when a caller supplies an occupied command id.
CREATE TRIGGER IF NOT EXISTS runtime_open_commands_collision
BEFORE INSERT ON runtime_events
WHEN EXISTS(SELECT 1 FROM runtime_events WHERE id=NEW.id
 AND type IN ('NATIVE_COMMAND_STARTED','NATIVE_COMMAND_FINISHED','NATIVE_COMMAND_RECOVERED'))
BEGIN
 INSERT INTO runtime_open_commands_dirty
 SELECT task_id FROM runtime_events WHERE id=NEW.id
 ON CONFLICT(task_id) DO NOTHING;
 DELETE FROM runtime_open_commands WHERE start_id=NEW.id;
END;

CREATE TRIGGER IF NOT EXISTS runtime_open_commands_update_collision
BEFORE UPDATE ON runtime_events WHEN OLD.id IS NOT NEW.id
 AND EXISTS(SELECT 1 FROM runtime_events WHERE id=NEW.id
  AND type IN ('NATIVE_COMMAND_STARTED','NATIVE_COMMAND_FINISHED','NATIVE_COMMAND_RECOVERED'))
BEGIN
 INSERT INTO runtime_open_commands_dirty
 SELECT task_id FROM runtime_events WHERE id=NEW.id
 ON CONFLICT(task_id) DO NOTHING;
 DELETE FROM runtime_open_commands WHERE start_id=NEW.id;
END;
