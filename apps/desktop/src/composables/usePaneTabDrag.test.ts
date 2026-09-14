// @vitest-environment happy-dom
import { mount } from '@vue/test-utils';
import { defineComponent, ref, nextTick } from 'vue';
import { expect, it, vi } from 'vitest';
import { usePaneTabDrag } from './usePaneTabDrag';

it('tracks a pointer gesture, ignores clicks, and cancels on scope change or Escape', async () => {
  const scope = ref('item:a');
  const moved = vi.fn();
  const wrapper = mount(defineComponent({
    setup() { const drag = usePaneTabDrag({ scope: () => scope.value, target: () => ({ paneId: 'two', beforeId: 'file:x' }), move: moved }); return { drag }; },
    template: `<div @pointerdown="drag.start($event, 'agent')">Agent<select><option>Latest</option></select></div>`,
  }), { attachTo: document.body });
  const fire = (el: EventTarget, type: string, x: number, y: number) => el.dispatchEvent(new PointerEvent(type, { bubbles: true, pointerId: 1, isPrimary: true, button: 0, clientX: x, clientY: y }));
  fire(wrapper.element, 'pointerdown', 10, 10);
  fire(document, 'pointerup', 11, 11);
  expect(moved).not.toHaveBeenCalled();
  fire(wrapper.element, 'pointerdown', 10, 10);
  fire(document, 'pointermove', 80, 20);
  fire(document, 'pointerup', 80, 20);
  expect(moved).toHaveBeenCalledExactlyOnceWith('agent', { paneId: 'two', beforeId: 'file:x' });
  moved.mockClear();
  fire(wrapper.get('select').element, 'pointerdown', 10, 10);
  fire(document, 'pointermove', 80, 20);
  fire(document, 'pointerup', 80, 20);
  expect(moved).not.toHaveBeenCalled();
  fire(wrapper.element, 'pointerdown', 10, 10);
  fire(document, 'pointermove', 80, 20);
  scope.value = 'item:b'; await nextTick();
  fire(document, 'pointerup', 80, 20);
  expect(moved).not.toHaveBeenCalled();
  fire(wrapper.element, 'pointerdown', 10, 10);
  fire(document, 'pointermove', 80, 20);
  document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape' }));
  fire(document, 'pointerup', 80, 20);
  expect(moved).not.toHaveBeenCalled();
  wrapper.unmount();
});
