import { useEffect, useMemo, useState } from "react";
import {
  acceptLocalRuleSuggestion,
  createLocalRule,
  deleteLocalRule,
  dismissLocalRuleSuggestion,
  getErrorMessage,
  getRulesPreferences,
  recomputeRulesProposal,
  reorderLocalRules,
  setLocalRuleEnabled,
  storeLocalOrganizationPreferences,
  updateLocalRule,
} from "./api";
import type {
  LocalRule,
  LocalRuleInput,
  OrganizationPreferences,
  RuleAction,
  RuleCondition,
  RuleField,
  RuleOperator,
  RulesPreferencesState,
  SemanticRuleField,
} from "./types";

interface RulesPreferencesViewProps {
  workspaceId: string;
}

type ActionKind = RuleAction["kind"];

const EMPTY_RULE: LocalRuleInput = {
  name: "",
  explanation: "",
  enabled: true,
  conditions: [
    {
      field: "document_type",
      operator: "equals",
      value: "invoice",
    },
  ],
  action: {
    kind: "prefer_project_location",
  },
};

const FIELD_OPTIONS: Array<{ value: RuleField; label: string }> = [
  { value: "document_type", label: "Type de document" },
  { value: "context", label: "Context" },
  { value: "supplier", label: "Supplier" },
  { value: "customer", label: "Customer" },
  { value: "project", label: "Project" },
  { value: "any_party", label: "Any party" },
  { value: "source_path", label: "Chemin source" },
];

function actionFor(kind: ActionKind): RuleAction {
  switch (kind) {
    case "set_semantic_field":
      return { kind, field: "document_type", value: "invoice" };
    case "classify_party":
      return { kind, party: "", role: "supplier" };
    case "set_destination":
      return { kind, segments: ["Business", "Administration"] };
    case "use_year_folders":
      return { kind, enabled: true };
    case "prefer_project_location":
    case "preserve_subtree":
      return { kind };
  }
}

function conditionLabel(condition: RuleCondition): string {
  const field =
    FIELD_OPTIONS.find((option) => option.value === condition.field)?.label ??
    condition.field;
  if (condition.operator === "exists") {
    return `${field} exists`;
  }
  return `${field} ${condition.operator.replace("_", " ")} ${condition.value ?? ""}`;
}

function actionLabel(action: RuleAction): string {
  switch (action.kind) {
    case "set_semantic_field":
      return `Set ${action.field.replace(/_/gu, " ")} to ${action.value}`;
    case "classify_party":
      return `Classify ${action.party} as ${action.role}`;
    case "prefer_project_location":
      return "Préférer l’emplacement du projet lié";
    case "set_destination":
      return `Use ${action.segments.join(" / ")}`;
    case "preserve_subtree":
      return "Conserver le sous-dossier actuel";
    case "use_year_folders":
      return action.enabled ? "Utiliser des dossiers par année" : "Ne pas utiliser des dossiers par année";
  }
}

function validDraft(value: LocalRuleInput): string | null {
  if (!value.name.trim() || !value.explanation.trim()) {
    return "Un nom et une raison en langage simple sont requis.";
  }
  if (value.conditions.length === 0) {
    return "Au moins une condition est requise.";
  }
  if (
    value.conditions.some((condition) =>
      condition.field === "source_path"
        ? condition.operator !== "starts_with"
        : condition.operator === "starts_with",
    )
  ) {
    return "Le préfixe ne s’applique qu’aux chemins sources.";
  }
  if (
    value.conditions.some(
      (condition) =>
        condition.operator !== "exists" && !condition.value?.trim(),
    )
  ) {
    return "Chaque condition « égal » ou « commence par » nécessite une valeur.";
  }
  if (
    value.action.kind === "set_destination" &&
    (value.action.segments.length === 0 ||
      value.action.segments.some(
        (segment) =>
          !segment.trim() ||
          segment === "." ||
          segment === ".." ||
          /[\\/:*?"<>|]/u.test(segment),
      ))
  ) {
    return "Les dossiers de destination doivent être des noms relatifs sûrs.";
  }
  if (
    value.action.kind === "set_semantic_field" &&
    !value.action.value.trim()
  ) {
    return "La valeur ne peut pas être vide.";
  }
  if (value.action.kind === "classify_party" && !value.action.party.trim()) {
    return "Le nom de la partie ne peut pas être vide.";
  }
  return null;
}

export function RulesPreferencesView({
  workspaceId,
}: RulesPreferencesViewProps) {
  const [state, setState] = useState<RulesPreferencesState | null>(null);
  const [draft, setDraft] = useState<LocalRuleInput>(EMPTY_RULE);
  const [editingId, setEditingId] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  async function load() {
    setState(await getRulesPreferences(workspaceId));
  }

  useEffect(() => {
    let active = true;
    void getRulesPreferences(workspaceId)
      .then((value) => {
        if (active) {
          setState(value);
        }
      })
      .catch((reason) => {
        if (active) {
          setError(getErrorMessage(reason));
        }
      });
    return () => {
      active = false;
    };
  }, [workspaceId]);

  const pendingSuggestions = useMemo(
    () => state?.suggestions.filter((suggestion) => suggestion.status === "pending") ?? [],
    [state],
  );

  async function mutate(label: string, operation: () => Promise<unknown>) {
    setBusy(label);
    setError(null);
    setNotice(null);
    try {
      await operation();
      await load();
    } catch (reason) {
      setError(getErrorMessage(reason));
    } finally {
      setBusy(null);
    }
  }

  async function saveRule(event: React.FormEvent) {
    event.preventDefault();
    const validation = validDraft(draft);
    if (validation) {
      setError(validation);
      return;
    }
    await mutate("rule", async () => {
      if (editingId) {
        await updateLocalRule(workspaceId, editingId, draft);
      } else {
        await createLocalRule(workspaceId, draft);
      }
      setDraft(EMPTY_RULE);
      setEditingId(null);
      setNotice("Rule saved locally. Recompute the preview when you are ready.");
    });
  }

  function editRule(rule: LocalRule) {
    setEditingId(rule.id);
    setDraft({
      name: rule.name,
      explanation: rule.explanation,
      enabled: rule.enabled,
      conditions: rule.conditions.map((condition) => ({ ...condition })),
      action:
        rule.action.kind === "set_destination"
          ? { ...rule.action, segments: [...rule.action.segments] }
          : { ...rule.action },
    });
    setError(null);
    setNotice(null);
  }

  async function moveRule(index: number, direction: -1 | 1) {
    if (!state) {
      return;
    }
    const destination = index + direction;
    if (destination < 0 || destination >= state.rules.length) {
      return;
    }
    const ordered = state.rules.map((rule) => rule.id);
    [ordered[index], ordered[destination]] = [
      ordered[destination],
      ordered[index],
    ];
    await mutate("reorder", () => reorderLocalRules(workspaceId, ordered));
  }

  async function savePreferences(event: React.FormEvent) {
    event.preventDefault();
    if (!state) {
      return;
    }
    await mutate("preferences", async () => {
      await storeLocalOrganizationPreferences(workspaceId, state.preferences);
      setNotice("Préférences enregistrées localement.");
    });
  }

  function updatePreferences(
    update: (current: OrganizationPreferences) => OrganizationPreferences,
  ) {
    setState((current) =>
      current
        ? { ...current, preferences: update(current.preferences) }
        : current,
    );
  }

  function updateCondition(index: number, patch: Partial<RuleCondition>) {
    setDraft((current) => ({
      ...current,
      conditions: current.conditions.map((condition, currentIndex) =>
        currentIndex === index
          ? {
              ...condition,
              ...patch,
              value:
                patch.operator === "exists"
                  ? null
                  : patch.value === undefined
                    ? condition.value
                    : patch.value,
            }
          : condition,
      ),
    }));
  }

  if (!state) {
    return (
      <section className="rules-preferences" aria-live="polite">
        <p>{error ?? "Chargement des règles et préférences…"}</p>
      </section>
    );
  }

  return (
    <section className="rules-preferences" aria-labelledby="rules-title">
      <header className="rules-heading">
        <div>
          <span className="eyebrow">Préférences</span>
          <h2 id="rules-title">Préférences de rangement</h2>
          <div className="rules-safety-banner" role="status">
            <strong>Suggestions uniquement.</strong>
            <span>
              Les règles influencent les futures propositions d’organisation.
              Elles n’autorisent pas à déplacer, renommer ou supprimer des
              fichiers sur le disque.
            </span>
          </div>
          <p>
            Les règles sont des instructions locales consultables. Elles peuvent
            influencer la compréhension, les propositions et le classement de
            recherche, mais elles n’appliquent jamais de modifications de
            fichiers.
          </p>
        </div>
        <button
          className="primary"
          type="button"
          disabled={busy !== null}
          onClick={() =>
            void mutate("recompute", async () => {
              const proposal = await recomputeRulesProposal(workspaceId);
              setNotice(
                proposal
                  ? `Preview recomputed as revision ${proposal.revision}; unrelated manual overrides were preserved.`
                  : "No current preview exists yet. Build one from Organization Preview.",
              );
            })
          }
        >
          {busy === "recompute" ? "RECALCUL…" : "RECALCULER L’APERÇU"}
        </button>
      </header>

      {error ? (
        <div className="error-banner" role="alert">
          <span>{error}</span>
          <button type="button" onClick={() => setError(null)}>
            Fermer
          </button>
        </div>
      ) : null}
      {notice ? (
        <p className="rules-notice" role="status">
          {notice}
        </p>
      ) : null}

      <div className="rules-grid">
        <div>
          <h3>Vos règles</h3>
          <p className="view-note">
            Les règles plus hautes l’emportent. Une décision manuelle sur un
            fichier reste prioritaire.
          </p>
          <div className="rule-list">
            {state.rules.map((rule, index) => (
              <article className={`rule-card${rule.enabled ? "" : " is-disabled"}`} key={rule.id}>
                <div className="rule-card__heading">
                  <div>
                    <strong>{rule.name}</strong>
                    <span>
                      {rule.origin === "accepted_suggestion"
                        ? "Suggestion acceptée"
                        : "Créée par vous"}
                    </span>
                  </div>
                  <label className="switch-label">
                    <input
                      aria-label={`Activer ${rule.name}`}
                      type="checkbox"
                      checked={rule.enabled}
                      disabled={busy !== null}
                      onChange={(event) =>
                        void mutate("toggle", () =>
                          setLocalRuleEnabled(
                            workspaceId,
                            rule.id,
                            event.target.checked,
                          ),
                        )
                      }
                    />
                    Enabled
                  </label>
                </div>
                <p>{rule.explanation}</p>
                <ul>
                  {rule.conditions.map((condition, conditionIndex) => (
                    <li key={`${rule.id}-${conditionIndex}`}>
                      IF {conditionLabel(condition)}
                    </li>
                  ))}
                  <li>THEN {actionLabel(rule.action)}</li>
                </ul>
                <div className="rule-actions">
                  <button
                    type="button"
                    disabled={busy !== null || index === 0}
                    onClick={() => void moveRule(index, -1)}
                  >
                    Move up
                  </button>
                  <button
                    type="button"
                    disabled={busy !== null || index === state.rules.length - 1}
                    onClick={() => void moveRule(index, 1)}
                  >
                    Move down
                  </button>
                  <button type="button" disabled={busy !== null} onClick={() => editRule(rule)}>
                    Edit
                  </button>
                  <button
                    className="danger-outline"
                    type="button"
                    disabled={busy !== null}
                    onClick={() =>
                      void mutate("delete", () =>
                        deleteLocalRule(workspaceId, rule.id),
                      )
                    }
                  >
                    Delete
                  </button>
                </div>
              </article>
            ))}
            {state.rules.length === 0 ? (
              <p className="empty-state">Aucune règle pour le moment. Créez-en une ci-dessous si vous voulez guider les suggestions.</p>
            ) : null}
          </div>
        </div>

        <form className="rule-editor" onSubmit={(event) => void saveRule(event)}>
          <h3>{editingId ? "Modifier la règle" : "Créer une règle"}</h3>
          <label>
            Nom de la règle
            <input
              value={draft.name}
              maxLength={120}
              onChange={(event) =>
                setDraft((current) => ({ ...current, name: event.target.value }))
              }
            />
          </label>
          <label>
            Why this rule exists
            <textarea
              value={draft.explanation}
              maxLength={512}
              rows={3}
              onChange={(event) =>
                setDraft((current) => ({
                  ...current,
                  explanation: event.target.value,
                }))
              }
            />
          </label>

          <fieldset>
            <legend>All conditions must match</legend>
            {draft.conditions.map((condition, index) => (
              <div className="condition-row" key={index}>
                <select
                  aria-label={`Condition ${index + 1} champ`}
                  value={condition.field}
                  onChange={(event) =>
                    updateCondition(index, {
                      field: event.target.value as RuleField,
                      operator:
                        event.target.value === "source_path"
                          ? "starts_with"
                          : condition.operator === "starts_with"
                            ? "equals"
                            : condition.operator,
                    })
                  }
                >
                  {FIELD_OPTIONS.map((option) => (
                    <option key={option.value} value={option.value}>
                      {option.label}
                    </option>
                  ))}
                </select>
                <select
                  aria-label={`Condition ${index + 1} opérateur`}
                  value={condition.operator}
                  onChange={(event) =>
                    updateCondition(index, {
                      operator: event.target.value as RuleOperator,
                    })
                  }
                >
                  {condition.field === "source_path" ? (
                    <option value="starts_with">starts with</option>
                  ) : (
                    <>
                      <option value="equals">equals</option>
                      <option value="exists">exists</option>
                    </>
                  )}
                </select>
                {condition.operator !== "exists" ? (
                  <input
                    aria-label={`Condition ${index + 1} valeur`}
                    value={condition.value ?? ""}
                    onChange={(event) =>
                      updateCondition(index, { value: event.target.value })
                    }
                  />
                ) : null}
                <button
                  type="button"
                  disabled={draft.conditions.length === 1}
                  onClick={() =>
                    setDraft((current) => ({
                      ...current,
                      conditions: current.conditions.filter(
                        (_, currentIndex) => currentIndex !== index,
                      ),
                    }))
                  }
                >
                  Remove
                </button>
              </div>
            ))}
            <button
              type="button"
              disabled={draft.conditions.length >= 8}
              onClick={() =>
                setDraft((current) => ({
                  ...current,
                  conditions: [
                    ...current.conditions,
                    {
                      field: "document_type",
                      operator: "equals",
                      value: "",
                    },
                  ],
                }))
              }
            >
              Add condition
            </button>
          </fieldset>

          <label>
            Action
            <select
              value={draft.action.kind}
              onChange={(event) =>
                setDraft((current) => ({
                  ...current,
                  action: actionFor(event.target.value as ActionKind),
                }))
              }
            >
              <option value="set_semantic_field">Set a semantic field</option>
              <option value="classify_party">Classify a party</option>
              <option value="prefer_project_location">Prefer project location</option>
              <option value="set_destination">Use a safe destination</option>
              <option value="preserve_subtree">Preserve subtree</option>
              <option value="use_year_folders">Set year-folder policy</option>
            </select>
          </label>
          <ActionEditor
            action={draft.action}
            onChange={(action) =>
              setDraft((current) => ({ ...current, action }))
            }
          />
          <div className="rule-actions">
            <button className="primary" type="submit" disabled={busy !== null}>
              {editingId ? "ENREGISTRER LA RÈGLE" : "CRÉER LA RÈGLE"}
            </button>
            {editingId ? (
              <button
                type="button"
                onClick={() => {
                  setEditingId(null);
                  setDraft(EMPTY_RULE);
                }}
              >
                Cancel edit
              </button>
            ) : null}
          </div>
        </form>
      </div>

      <form className="preferences-form" onSubmit={(event) => void savePreferences(event)}>
        <div>
          <h3>Organization preferences</h3>
          <p className="view-note">
            Les préférences sont moins prioritaires que les champs confirmés et
            les règles explicites.
          </p>
        </div>
        <div className="preferences-grid">
          <label>
            Personal root
            <input
              value={state.preferences.personalRootName}
              onChange={(event) =>
                updatePreferences((current) => ({
                  ...current,
                  personalRootName: event.target.value,
                }))
              }
            />
          </label>
          <label>
            Business root
            <input
              value={state.preferences.businessRootName}
              onChange={(event) =>
                updatePreferences((current) => ({
                  ...current,
                  businessRootName: event.target.value,
                }))
              }
            />
          </label>
          <label>
            Folder language
            <select
              value={state.preferences.namingLanguage}
              onChange={(event) =>
                updatePreferences((current) => ({
                  ...current,
                  namingLanguage: event.target.value as "en" | "fr",
                }))
              }
            >
              <option value="en">English</option>
              <option value="fr">Français</option>
            </select>
          </label>
          <label>
            Maximum depth
            <input
              type="number"
              min={2}
              max={8}
              value={state.preferences.maximumDepth}
              onChange={(event) =>
                updatePreferences((current) => ({
                  ...current,
                  maximumDepth: Number(event.target.value),
                }))
              }
            />
          </label>
          <label>
            Minimum folder group
            <input
              type="number"
              min={1}
              max={20}
              value={state.preferences.minimumGroupSize}
              onChange={(event) =>
                updatePreferences((current) => ({
                  ...current,
                  minimumGroupSize: Number(event.target.value),
                }))
              }
            />
          </label>
          <label>
            Review threshold ({Math.round(state.preferences.reviewThreshold * 100)}%)
            <input
              type="range"
              min={0.5}
              max={0.99}
              step={0.01}
              value={state.preferences.reviewThreshold}
              onChange={(event) =>
                updatePreferences((current) => ({
                  ...current,
                  reviewThreshold: Number(event.target.value),
                }))
              }
            />
          </label>
          <label className="wide-field">
            Safe rename template
            <input
              value={state.preferences.renameTemplate}
              onChange={(event) =>
                updatePreferences((current) => ({
                  ...current,
                  renameTemplate: event.target.value,
                }))
              }
            />
            <small>
              Tokens: {"{date}"}, {"{party}"}, {"{document_type}"},{" "}
              {"{identifier}"}, {"{project}"}, {"{original}"}
            </small>
          </label>
          {[
            ["includeYearFolders", "Use year folders"],
            ["clientFirst", "Client-first hierarchy"],
            ["keepPhotosInsideProjects", "Keep project photos inside projects"],
            [
              "supplierInvoicesInsideProjects",
              "Keep linked supplier invoices inside projects",
            ],
            ["preserveExistingFolders", "Preserve useful existing folders"],
          ].map(([key, label]) => (
            <label className="switch-label" key={key}>
              <input
                type="checkbox"
                checked={Boolean(
                  state.preferences[key as keyof OrganizationPreferences],
                )}
                onChange={(event) =>
                  updatePreferences((current) => ({
                    ...current,
                    [key]: event.target.checked,
                  }))
                }
              />
              {label}
            </label>
          ))}
        </div>
        <button className="primary" type="submit" disabled={busy !== null}>
          ENREGISTRER LES PRÉFÉRENCES
        </button>
      </form>

      <section className="suggestions-panel">
        <h3>Suggestions à confirmer</h3>
        <p className="view-note">
          Les corrections répétées créent seulement des suggestions. Rien ne
          devient une règle automatiquement, et aucune donnée d’apprentissage
          ne quitte cet appareil.
        </p>
        {pendingSuggestions.map((suggestion) => (
          <article className="suggestion-card" key={suggestion.id}>
            <div>
              <strong>{suggestion.title}</strong>
              <span>{suggestion.evidenceCount} corrections locales correspondantes</span>
            </div>
            <p>{suggestion.explanation}</p>
            <p>
              Comportement proposé : {suggestion.proposedRule.explanation}
            </p>
            <div className="rule-actions">
              <button
                className="primary"
                type="button"
                disabled={busy !== null}
                onClick={() =>
                  void mutate("accept", () =>
                    acceptLocalRuleSuggestion(workspaceId, suggestion.id),
                  )
                }
              >
                ACCEPTER ET CRÉER LA RÈGLE
              </button>
              <button
                type="button"
                disabled={busy !== null}
                onClick={() =>
                  void mutate("dismiss", () =>
                    dismissLocalRuleSuggestion(workspaceId, suggestion.id),
                  )
                }
              >
                Ignorer
              </button>
            </div>
          </article>
        ))}
        {pendingSuggestions.length === 0 ? (
          <p className="empty-state">Aucune suggestion en attente.</p>
        ) : null}
      </section>
    </section>
  );
}

function ActionEditor({
  action,
  onChange,
}: {
  action: RuleAction;
  onChange: (value: RuleAction) => void;
}) {
  switch (action.kind) {
    case "set_semantic_field":
      return (
        <div className="condition-row">
          <select
            aria-label="Semantic field"
            value={action.field}
            onChange={(event) =>
              onChange({
                ...action,
                field: event.target.value as SemanticRuleField,
              })
            }
          >
            <option value="document_type">Document type</option>
            <option value="context">Context</option>
            <option value="supplier">Supplier</option>
            <option value="customer">Customer</option>
            <option value="project">Project</option>
          </select>
          <input
            aria-label="Semantic value"
            value={action.value}
            onChange={(event) =>
              onChange({ ...action, value: event.target.value })
            }
          />
        </div>
      );
    case "classify_party":
      return (
        <div className="condition-row">
          <input
            aria-label="Party name"
            value={action.party}
            onChange={(event) =>
              onChange({ ...action, party: event.target.value })
            }
          />
          <select
            aria-label="Party role"
            value={action.role}
            onChange={(event) =>
              onChange({
                ...action,
                role: event.target.value as "supplier" | "customer",
              })
            }
          >
            <option value="supplier">Supplier</option>
            <option value="customer">Customer</option>
          </select>
        </div>
      );
    case "set_destination":
      return (
        <label>
          Destination (separate folders with /)
          <input
            value={action.segments.join(" / ")}
            onChange={(event) =>
              onChange({
                ...action,
                segments: event.target.value
                  .split("/")
                  .map((segment) => segment.trim()),
              })
            }
          />
        </label>
      );
    case "use_year_folders":
      return (
        <label className="switch-label">
          <input
            type="checkbox"
            checked={action.enabled}
            onChange={(event) =>
              onChange({ ...action, enabled: event.target.checked })
            }
          />
          Use year folders for matching files
        </label>
      );
    case "prefer_project_location":
    case "preserve_subtree":
      return null;
  }
}
