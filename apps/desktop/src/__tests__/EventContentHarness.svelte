<!-- Test harness: provides the adapter's context to EventContent. -->
<script lang="ts">
  import { setContext } from "svelte";
  import EventContent from "../components/calendar/EventContent.svelte";
  import { EVENT_CONTEXT, type EventContext, type SvarEvent } from "../components/calendar/svarMapping";

  let { event, mode, open }: { event: SvarEvent; mode: string; open: (id: string) => void } = $props();
  setContext<EventContext>(EVENT_CONTEXT, { calendarName: () => "Work", open: (id) => open(id) });
</script>

<EventContent {event} {mode} />
