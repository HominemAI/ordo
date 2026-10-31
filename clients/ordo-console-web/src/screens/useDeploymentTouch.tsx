import { useRef } from 'react';
import { needsDeploymentTouch, touchDeployment } from '../deployment';
import { withTouch } from '../touch';

/**
 * Run a call at the deployment. When the console asks for the owner's own
 * touch, take it; when the deployment asks for the owner's touch there,
 * ask in a dialog, relay it, and run the call again.
 */
export function useDeploymentTouch() {
  const dialog = useRef<HTMLDialogElement>(null);
  const pending = useRef<{ resolve: () => void; reject: (e: unknown) => void } | null>(null);

  const ask = () =>
    new Promise<void>((resolve, reject) => {
      pending.current = { resolve, reject };
      dialog.current?.showModal();
    });

  const cancel = () => {
    pending.current?.reject(new DOMException('The touch was cancelled.', 'AbortError'));
    pending.current = null;
  };

  const touchNow = async () => {
    const p = pending.current;
    pending.current = null;
    try {
      await touchDeployment();
      dialog.current?.close();
      p?.resolve();
    } catch (e) {
      dialog.current?.close();
      p?.reject(e);
    }
  };

  const atDeployment = async <T,>(f: () => Promise<T>): Promise<T> => {
    try {
      return await withTouch(f);
    } catch (e) {
      if (!needsDeploymentTouch(e)) throw e;
    }
    await ask();
    return withTouch(f);
  };

  const prompt = (
    <dialog ref={dialog} aria-labelledby="deploy-touch-title" onCancel={cancel}>
      <h2 id="deploy-touch-title">Touch your passkey for the deployment</h2>
      <p>
        This is done at the deployment, which asks for your own passkey there. One touch covers the next 5 minutes.
      </p>
      <div className="actions">
        <button type="button" className="primary" onClick={() => void touchNow()}>
          Use my passkey
        </button>
        <button
          type="button"
          className="link"
          onClick={() => {
            dialog.current?.close();
            cancel();
          }}
        >
          Cancel
        </button>
      </div>
    </dialog>
  );

  return { atDeployment, prompt };
}
