import type {OutputMarks} from './types'
export type LayoutSettings={max_pages_per_volume:number;keep_groups_intact:boolean;output_marks:OutputMarks}
export type BundlePreset={id:string;name:string;version:number;created_at?:string;provenance?:string;settings:LayoutSettings}
export type SavedLayout=LayoutSettings&{applied:BundlePreset|null;input_snapshot:string}
export const matterOnly=['volume_splits','annexure_overrides','cover_heading','cover_reference','cover_title']
export function presetMarks(marks:OutputMarks):OutputMarks{return {...marks,volume_splits:{},annexure_overrides:{},cover_heading:'',cover_reference:'',cover_title:''}}
export function differences(settings:LayoutSettings,applied:BundlePreset|null){
 if(!applied)return []
 const result:Array<{field:string;label:string;baseline:unknown;current:unknown}>=[]
 const names:Record<string,string>={max_pages_per_volume:'Maximum pages per volume',keep_groups_intact:'Keep groups together',front_matter:'Volume front matter',index_date:'Index date column',index_style:'Index layout'}
 for(const key of ['max_pages_per_volume','keep_groups_intact'] as const)if(settings[key]!==applied.settings[key])result.push({field:key,label:names[key],baseline:applied.settings[key],current:settings[key]})
 const baseline=Object.assign({volume_max_bytes:null,volume_splits:{},processing_quality:'original',cleanup_metadata:false,print_layout:'screen',office_docx:false,office_xlsx:false,office_pptx:false,front_matter:'index_only',index_date:false,numbering_style:'arabic',numbering_restart:'volume',numbering_start:1,numbering_prefix:'',numbering_position:'bottom_center',numbering_front_matter:'physical',numbering_pdf_labels:true,numbering_linked_index:true,numbering_back_link:false},applied.settings.output_marks) as unknown as Record<string,unknown>
 for(const [key,value] of Object.entries(settings.output_marks))if(!matterOnly.includes(key)&&JSON.stringify(value)!==JSON.stringify(baseline[key]))result.push({field:key,label:names[key]||key.replace(/_/g,' '),baseline:baseline[key],current:value})
 return result
}
export function displaySetting(value:unknown){return typeof value==='boolean'?(value?'On':'Off'):String(value??'Default').replace(/_/g,' ')}
