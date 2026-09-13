// @vitest-environment happy-dom
import { describe, expect, it } from 'vitest';
import { mount } from '@vue/test-utils';
import TaskPreviewCache from '../TaskPreviewCache.vue';

const entry = (key: string, left: string) => ({ key, taskId:'task', portName:key, workspace:'/worktree', supported:true, style:{left} });
describe('preview pane continuity', () => {
  it('keeps the same mounted views while panes move and discards superseded workspaces', async () => {
    const wrapper = mount(TaskPreviewCache, {
      props:{visibleEntries:[entry('one','0%'),entry('two','50%')],workspaces:{task:'/worktree'}},
      global:{stubs:{TaskPreviewView:{props:['portName'],template:'<section :data-port="portName"><input></section>'}}},
    });
    const one = wrapper.get('[data-port="one"]').element;
    const two = wrapper.get('[data-port="two"]').element;
    (one.querySelector('input') as HTMLInputElement).value='keep this draft';
    await wrapper.setProps({visibleEntries:[entry('two','0%'),entry('one','50%')]});
    expect(wrapper.get('[data-port="one"]').element).toBe(one);
    expect(wrapper.get('[data-port="two"]').element).toBe(two);
    expect((one.querySelector('input') as HTMLInputElement).value).toBe('keep this draft');
    await wrapper.setProps({visibleEntries:[]});
    expect(wrapper.get('[data-port="one"]').attributes('style')).toContain('display: none');
    await wrapper.setProps({visibleEntries:[entry('one','0%')]});
    expect(wrapper.get('[data-port="one"]').element).toBe(one);
    await wrapper.setProps({visibleEntries:[],workspaces:{task:'/new-worktree'}});
    expect(wrapper.find('[data-port="one"]').exists()).toBe(false);
    expect(wrapper.find('[data-port="two"]').exists()).toBe(false);
    wrapper.unmount();
  });
});
