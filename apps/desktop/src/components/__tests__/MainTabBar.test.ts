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
describe('tab dragging', () => {
  it('moves an agent tab only within its own task scope', async () => {
    const wrapper = mount(MainTabBar, { props: { tabs:[{id:'agent',kind:'agent'}], activeTabId:'agent', paneId:'pane-2', scopeKey:'item:a' } });
    const dataTransfer = new DataTransfer();
    dataTransfer.setData('application/x-kanna-tab', JSON.stringify({id:'agent',scope:'item:b'}));
    await wrapper.get('.main-tab-bar').trigger('drop',{dataTransfer});
    expect(wrapper.emitted('dropTab')).toBeUndefined();
    dataTransfer.setData('application/x-kanna-tab', JSON.stringify({id:'agent',scope:'item:a'}));
    await wrapper.get('.main-tab-bar').trigger('drop',{dataTransfer});
    expect(wrapper.emitted('dropTab')).toEqual([['agent',undefined]]);
    wrapper.unmount();
  });
});

it('keeps content creation separate from the pane context menu, with keyboard access', async () => {
  const wrapper = mount(MainTabBar, { attachTo: document.body, props: {
    tabs: [{ id: 'agent', kind: 'agent' }], activeTabId: 'agent', paneId: 'pane-1',
    newViews: [{ id: 'file', label: 'Open file…' }],
    paneActions: [{ id: 'split-horizontal', label: 'Split side by side' }, { id: 'join', label: 'Join panes' }],
  } });
  await wrapper.get('.new-tab').trigger('click');
  await flushPromises();
  expect(document.querySelector('.new-tab-menu')?.textContent).toBe('Open file…');
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
