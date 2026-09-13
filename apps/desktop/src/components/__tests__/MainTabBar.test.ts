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
