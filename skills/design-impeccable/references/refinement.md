# Refinement reference (write roles)

Cortex adaptation of the polish, bolder, quieter, distill, typeset, layout and
clarify passes in the Impeccable skill. Apply each pass to the named target
only. Re-check the same states and viewports after every pass.

## Shared rules

- Refinement preserves the incumbent identity, behavior, copy and everything
  outside the target. If the concept itself is wrong, say so and recommend a
  separate design-direction task instead of smuggling in a replacement.
- Ask before changing factual copy, claims or numbers.
- Do not add a color, font, radius, shadow or system primitive the surface does
  not already own unless the brief asks for it. Name the addition and its job
  if one is needed.
- Verify on the rendered result at desktop and mobile widths, and open the
  captured images before and after.
- A written verification answer cites the rendered or source evidence for it.
  A bare "yes" is not verification.

## polish: final consistency pass

1. Read the project's design documentation, tokens, shared components and
   neighboring flows. Classify each drift (missing token, one-off implementation,
   conceptual mismatch, local defect) and fix it at the narrowest correct level.
2. Walk the complete path with mouse, keyboard and touch where applicable.
3. Check loading, empty, error, success, disabled and long-content states.
4. Align to the project's grid and spacing scale, including optical alignment.
   Keep same-role typography consistent and test wrapping, localization and zoom.
5. Give every control default, hover, focus, active, disabled, loading, error and
   success styling. Keep focus visible and touch targets appropriately sized.
6. Keep motion coherent and interruptible. Do not add motion just to show polish.
7. Remove debug output, dead code, unused styles and polish-created duplication.
   Promote a value to a token only if it is reused.

## bolder: raise one flat section

- A section reads flat when it quietly skips the strongest moves its neighbors
  already make: display type at full strength, meaningful structural devices,
  the signature motif, the page's density and pacing.
- Amplify what the system already owns. Use its motif and type scale turned up
  for this section. The result must look like the same brand, only more certain.
- Commit to one decisive move, then quiet the surroundings so the move reads.
- Give the section its own rhythm: a peak in the scroll, not more of the same.
- Strip the copy from the planned section and check the bare structure. If the
  section only works once words return, the boldness lives in the text size.
- Reject the reflex of adding effects. More effects are usually less bold.

## quieter: reduce intensity

- Identify intensity sources: saturation, contrast extremes, heavy weights, large
  scale with no hierarchy, excess motion, decorative layers.
- Desaturate toward a restrained palette, let neutrals carry more of the layout,
  and use accent color sparingly.
- Reduce weights one step where it does not hurt hierarchy. Use size, spacing
  and weight for hierarchy instead of color and boldness.
- Remove gradients, glows and multiple shadows that serve no purpose. Shorten
  motion distances and use gentle easing; remove decorative motion.
- Keep personality and the functional affordances. Quiet is not grayscale and
  not generic.

## distill: strip to the essence

- Name the one primary goal of the surface. Keep what is necessary to reach it.
- Remove redundant copy, repeated information, competing secondary actions and
  unnecessary containers. Do not nest cards inside cards.
- Use one spacing scale and one alignment direction.
- Progressive disclosure for optional features; never remove functionality,
  accessible labels or the information needed for a decision.
- Document what was removed and where an alternative access point remains.

## typeset: type hierarchy

1. **Assess the typography.** Answer each question with a file, selector or
   computed value:
   - Authority and fit: which faces, weights and roles are established, and do
     they fit the product, or are they unexamined defaults? Is each family necessary?
   - Hierarchy: can heading, body, label, metadata and data roles be told apart
     at a glance? Are adjacent sizes or weights too close to carry different jobs?
   - Scale and consistency: is there a deliberate role scale, or a set of arbitrary
     values? Do repeated roles stay identical across screens and states?
   - Reading: does body copy stay within about 45 to 75 characters per line? Are
     line height, paragraph rhythm and contrast tuned to the actual face and surface?
   - Stress: long headings, localization expansion, zoom, narrow containers,
     missing weights and font fallback.
   - Delivery: only used font assets and weights are loaded; fallback metrics and
     loading avoid invisible text and disruptive reflow.
2. **Set the system before editing.** State the roles the interface needs, the
   contrast between them, the reading measure and density, the authoritative
   faces and weights, and any performance, localization or accessibility limits.
   Use the fewest roles and families that make the hierarchy clear. Name tokens
   by purpose, not value.
3. **Apply.**
   - Body text at 1rem (16px) as the ordinary web floor, unless a dense role,
     platform convention or user setting justifies otherwise.
   - Tune line height to measure: wider lines need more leading. Tune it to the
     face, width, language and contrast, not one universal ratio.
   - Light text on dark surfaces may need slightly more line height, a little more
     tracking and sometimes one step more weight.
   - Use numeric, tabular, code and label font features where the content benefits.
   - Preserve browser zoom, user font settings and platform text scaling.
   - Use paragraph spacing or first-line indentation for rhythm, not both.
   - Do not make type decorative at the expense of comprehension, and do not add a
     second family without a role only it can perform.
4. **Verify.** Primary, secondary, body and metadata roles are recognizable without
   reading the copy; long text stays comfortable at relevant widths and languages;
   loading causes no invisible text or disruptive reflow; zoom and focus remain
   usable. Re-check the rendered result.

## layout: rhythm and hierarchy

1. **Assess the layout.** Answer each question with rendered or source evidence:
   - Reading order: with detail blurred, can the primary element, the secondary
     element and the major groups still be identified in order?
   - Grouping: are related items close and distinct groups separated, or are
     containers compensating for weak proximity?
   - Rhythm: do tight and generous intervals create a deliberate cadence, or is one
     spacing value repeated until everything weighs the same?
   - Structure: do repeated cards, columns or sections match the content, or are
     they a framework default?
   - Density: does information per region fit the use frequency and the
     decision complexity?
   - Adaptation: what reorders, collapses, wraps or scrolls at narrow,
     intermediate, wide and zoomed widths? Do DOM order, focus order and visual
     order still agree?
   - Extremes: long content, empty states, overlays, sticky elements, safe areas
     and small touch targets.
2. **Set the spatial thesis before editing.** Name the primary reading or task
   path, what belongs together, what must separate, which element leads, the
   intended density and rhythm, and how the structure changes across widths.
   Choose the simplest structural model that expresses those relationships.
3. **Apply.**
   - Group by meaning first; use proximity before adding containers or decoration.
   - Create rhythm through deliberate contrast between tight and generous intervals.
   - Use a documented spacing scale rather than one-off values.
   - Make responsive behavior structural: reorder, collapse or reveal by what
     remains important. Keep touch targets usable even when marks are small.
   - Use depth only when it clarifies state or hierarchy.
   - Make optical corrections only after inspecting the rendered result.
4. **Verify.** The primary, secondary and major groups still read in order at every
   supported size; related content groups together; long text, empty states,
   localization and zoom do not break the structure; keyboard and assistive-technology
   order match the visual order.

## clarify: plain-language copy

1. **Audit the language across the whole path**, not isolated strings. Look for
   ambiguous nouns and verbs, internal jargon, vague outcomes and system states,
   missing consequences or recovery, inconsistent terms and capitalization,
   redundant headings and confirmations, and text that breaks at realistic widths
   or in translation.
2. **Set the message hierarchy** for each state: the one fact the user needs now,
   the next action, the context that changes the decision, and the tone for the
   moment. Say each idea once.
3. **Rewrite by function.**
   - Actions: name the outcome with a specific verb and object. Keep one name for
     one concept across the product. For destructive actions, name the object and
     the consequence; prefer undo to confirmation when recovery is safe. Do not use
     "Yes", "No", "OK" or "Submit" as the only label.
   - Forms: persistent labels, with placeholders as examples only. Put format and
     eligibility rules before submission. Errors say what needs attention and how
     to fix it, placed near the field and announced to assistive technology.
   - Errors and permissions: say what failed, why when it is known and useful, and
     how to recover. Do not lead with internal codes or promise a cause the system
     cannot know.
   - Loading, empty and success: loading names the real operation and shows real
     progress only; empty states distinguish first use, no results, filters,
     permissions and failure, and offer the next action; success confirms the
     outcome briefly.
   - Help: helper text answers the question the user has, not a restatement of the
     control. Link text must make sense out of context; icon-only controls need names.
4. **Localize and make accessible.** Write complete translatable sentences rather
   than fragments. Keep variables and numbers structured so translators can reorder
   them. Allow text expansion. Keep screen-reader names aligned with visible labels.
   Do not rely on color or iconography alone to carry a message.
5. **Verify** comprehension without hidden product knowledge, actionability at
   errors and empty states, factual accuracy, scanability at target widths and 200%
   zoom, long names and pluralization, accessible names, and a tone that matches
   the consequence. Keep the final copy as short as possible without losing meaning
   or recovery.
