import { useEffect, useState, type FormEvent } from "react";

type Props = {
  name?: string;
  tags: string[];
  label: string;
  disabled: boolean;
  onSave: (name: string | undefined, tags: string[]) => void;
};

export default function AnnotationEditor({
  name,
  tags,
  label,
  disabled,
  onSave,
}: Props) {
  const [editedName, setEditedName] = useState(name ?? "");
  const [editedTags, setEditedTags] = useState(tags.join(", "));

  useEffect(() => {
    setEditedName(name ?? "");
    setEditedTags(tags.join(", "));
  }, [name, tags]);

  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    onSave(
      name === undefined ? undefined : editedName,
      editedTags.split(",").map((tag) => tag.trim()).filter(Boolean),
    );
  }

  return (
    <form className="annotation-editor" aria-label={label} onSubmit={submit}>
      {name !== undefined && (
        <label>
          Asset-Name
          <input
            value={editedName}
            onChange={(event) => setEditedName(event.target.value)}
            maxLength={256}
            required
            disabled={disabled}
          />
        </label>
      )}
      <label>
        {name === undefined ? "Paket-Tags" : "Asset-Tags"}
        <input
          value={editedTags}
          onChange={(event) => setEditedTags(event.target.value)}
          placeholder="z. B. fluss, ambience"
          disabled={disabled}
        />
      </label>
      <p className="detail-hint">
        Tags mit Komma trennen.
      </p>
      <button
        className="button outlined"
        type="submit"
        disabled={disabled || (editedName === (name ?? "") && editedTags === tags.join(", "))}
      >
        Speichern
      </button>
    </form>
  );
}
