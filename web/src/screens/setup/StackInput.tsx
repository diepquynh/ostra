import { useId } from "react";
import { Input, type InputProps } from "../../design";

/** Free-text stack, suggesting the stacks the server has a seed reference for. Empty means detect from the code. */
export function StackInput({ stacks, ...rest }: InputProps & { stacks: string[] }) {
  const listId = useId();
  return (
    <>
      <Input list={listId} autoComplete="off" placeholder="Detect from the code" {...rest} />
      <datalist id={listId}>
        {stacks.map((s) => (
          <option key={s} value={s} />
        ))}
      </datalist>
    </>
  );
}
