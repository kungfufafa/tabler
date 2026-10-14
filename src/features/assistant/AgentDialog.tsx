import { useState, type FormEvent } from "react";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Textarea } from "@/components/ui/textarea";
import type { AssistantProvider } from "@/lib/ipc";
import { agentError, MAX_AGENT_NAME, useAgentsStore, type Agent } from "./agents";
import { ASSISTANT_PROVIDERS, PROVIDER_LABELS } from "./providers";

const DEFAULT_PROVIDER = "default";

/** New Agent… / Edit Agent…: a name, instructions, and an optional provider / model. */
export function AgentDialog({
  agent,
  open,
  onOpenChange,
}: {
  /** The agent edited; null creates one (prefilled from `template` when given). */
  agent: Agent | null;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const save = useAgentsStore((s) => s.save);
  const [name, setName] = useState(agent?.builtIn ? `${agent.name} copy` : (agent?.name ?? ""));
  const [prompt, setPrompt] = useState(agent?.systemPrompt ?? "");
  const [provider, setProvider] = useState<AssistantProvider | null>(agent?.provider ?? null);
  const [model, setModel] = useState(agent?.model ?? "");
  const [error, setError] = useState<string | null>(null);
  const editing = agent !== null && !agent.builtIn;

  const submit = (e: FormEvent) => {
    e.preventDefault();
    const input = { name, systemPrompt: prompt, provider, model: model.trim() || null };
    const problem = agentError(input);
    if (problem) {
      setError(problem);
      return;
    }
    try {
      save(input, editing ? agent.id : undefined);
      onOpenChange(false);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-lg">
        <form onSubmit={submit} className="flex flex-col gap-3">
          <DialogHeader>
            <DialogTitle>{editing ? "Edit Agent" : "New Agent"}</DialogTitle>
            <DialogDescription>
              Instructions the assistant follows in this agent’s conversations. The structure of the
              current connection is added after them.
            </DialogDescription>
          </DialogHeader>
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="agent-name" className="text-xs">
              Name
            </Label>
            <Input
              id="agent-name"
              autoFocus
              value={name}
              maxLength={MAX_AGENT_NAME}
              onChange={(e) => setName(e.target.value)}
              className="h-7 text-xs"
            />
          </div>
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="agent-prompt" className="text-xs">
              Instructions
            </Label>
            <Textarea
              id="agent-prompt"
              value={prompt}
              rows={8}
              onChange={(e) => setPrompt(e.target.value)}
              placeholder="e.g. You review SQL migrations for PostgreSQL and point out locking risks."
              className="max-h-72 min-h-32 font-mono text-xs md:text-xs"
            />
          </div>
          <div className="grid grid-cols-2 gap-3">
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="agent-provider" className="text-xs">
                Provider
              </Label>
              <Select
                value={provider ?? DEFAULT_PROVIDER}
                onValueChange={(v) =>
                  setProvider(v === DEFAULT_PROVIDER ? null : (v as AssistantProvider))
                }
              >
                <SelectTrigger id="agent-provider" size="sm" className="h-7 w-full text-xs">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent data-scope="dialog">
                  <SelectItem value={DEFAULT_PROVIDER} className="text-xs">
                    Default (Settings)
                  </SelectItem>
                  {ASSISTANT_PROVIDERS.map((p) => (
                    <SelectItem key={p} value={p} className="text-xs">
                      {PROVIDER_LABELS[p]}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="agent-model" className="text-xs">
                Model
              </Label>
              <Input
                id="agent-model"
                value={model}
                spellCheck={false}
                placeholder="Provider's model"
                onChange={(e) => setModel(e.target.value)}
                className="h-7 font-mono text-xs"
              />
            </div>
          </div>
          {error && (
            <p role="alert" className="text-destructive text-xs">
              {error}
            </p>
          )}
          <DialogFooter>
            <Button type="button" variant="outline" size="sm" onClick={() => onOpenChange(false)}>
              Cancel
            </Button>
            <Button type="submit" size="sm">
              {editing ? "Save" : "Create"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
