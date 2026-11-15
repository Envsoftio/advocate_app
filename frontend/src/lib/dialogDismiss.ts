import type { ObjectDirective } from 'vue';

// Backdrop dismissal goes through the same cancel path as Escape, preserving
// each workflow's busy checks and Save / Discard / Keep editing guards.
export const dismissOutside: ObjectDirective<HTMLDialogElement> = {
  mounted(dialog) {
    let startedOutside = false;
    let previousFocus: HTMLElement | null = null;
    const outside = (event: MouseEvent) => {
      const rect = dialog.getBoundingClientRect();
      return (
        event.target === dialog &&
        (event.clientX < rect.left ||
          event.clientX > rect.right ||
          event.clientY < rect.top ||
          event.clientY > rect.bottom)
      );
    };
    dialog.addEventListener('pointerdown', (event) => {
      startedOutside = outside(event);
      if (startedOutside) previousFocus = dialog.ownerDocument.activeElement as HTMLElement | null;
    });
    dialog.addEventListener('click', (event) => {
      if (!startedOutside || !outside(event)) return;
      startedOutside = false;
      previousFocus?.focus({ preventScroll: true });
      if (dialog.dispatchEvent(new Event('cancel', { cancelable: true }))) dialog.close();
    });
  },
};
