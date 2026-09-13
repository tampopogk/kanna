// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { mount, flushPromises } from '@vue/test-utils';
import MainTabBar from '../MainTabBar.vue';
vi.mock('vue-i18n', () => ({ useI18n: () => ({ t: (key: string) => key }) }));
afterEach(() => { document.body.innerHTML = ''; });
it('opens views from the plus menu with keyboard navigation and dismissal', async () => {
  const wrapper = mount(MainTabBar, { attachTo: document.body, props: { tabs: [{id:'agent',kind:'agent'}], activeTabId:'agent', newViews:[{id:'file',label:'Open file…'},{id:'shell',label:'Terminal'}] } });
  await wrapper.get('.new-tab').trigger('click');
  await flushPromises();
  expect(document.activeElement?.textContent).toBe('Open file…');
  document.activeElement?.dispatchEvent(new KeyboardEvent('keydown', {key:'ArrowDown',bubbles:true}));
  expect(document.activeElement?.textContent).toBe('Terminal');
  (document.activeElement as HTMLButtonElement).click();
  expect(wrapper.emitted('new')).toEqual([['shell']]);
  await flushPromises();
  expect(document.querySelector('.new-tab-menu')).toBeNull();
  wrapper.unmount();
});
it('keeps content creation separate from the pane context menu, with keyboard access', async () => {
  const wrapper = mount(MainTabBar, { attachTo: document.body, props: {
    tabs: [{ id: 'agent', kind: 'agent' }], activeTabId: 'agent', paneId: 'pane-1',
    newViews: [{ id: 'file', label: 'Open file…', shortcut: '⌘P' }],
    paneActions: [{ id: 'split-horizontal', label: 'Split side by side' }],
  } });
  await wrapper.get('.new-tab').trigger('click');
  await flushPromises();
  expect(document.querySelector('.new-tab-menu')?.textContent).toBe('Open file…⌘P');
  expect(document.querySelector('.new-tab-menu kbd')?.textContent).toBe('⌘P');
  await wrapper.get('.main-tab-bar').trigger('contextmenu', { clientX: 20, clientY: 30 });
  await flushPromises();
  expect(document.querySelector('.new-tab-menu')).toBeNull();
  expect(document.activeElement?.textContent).toBe('Split side by side');
  (document.activeElement as HTMLButtonElement).click();
  expect(wrapper.emitted('layout')).toEqual([['split-horizontal']]);
  expect(wrapper.emitted('new')).toBeUndefined();
  await wrapper.get('.main-tab-bar').trigger('keydown', { key: 'F10', shiftKey: true });
  await flushPromises();
  document.activeElement?.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
  await flushPromises();
  expect(document.querySelector('.pane-layout-menu')).toBeNull();
  expect(document.activeElement).toBe(wrapper.get('.main-tab-bar').element);
  wrapper.unmount();
});

it('closes only its pane through a separate control', async () => {
  const wrapper = mount(MainTabBar, { props: { tabs: [{id:'agent',kind:'agent'}], activeTabId:'agent', canClosePane:true } });
  await wrapper.get('.close-pane').trigger('click');
  expect(wrapper.emitted('closePane')).toEqual([[]]);
  expect(wrapper.emitted('close')).toBeUndefined();
  await wrapper.setProps({canClosePane:false});
  expect(wrapper.find('.close-pane').exists()).toBe(false);
  wrapper.unmount();
});
